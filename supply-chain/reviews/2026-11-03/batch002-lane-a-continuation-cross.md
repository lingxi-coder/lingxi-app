# Independent cross-review — lane A continuation, three exact versions

Reviewer: Codex, AI-assisted source review, 2026-11-03. This independently evaluates the three proposals in `../lane-a/continuation.md`; it does not register audits, change exemptions, or certify the whole dependency graph. The inspected App lock SHA-256 is `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`; the all-features metadata SHA-256 is `ad6283c5ee42e875a3c542bd268a319266c5162c647e74290a397ef1ee1054da` (metadata contains local paths).

## Independent identity and coverage

Downloaded each exact `.crate` separately from `https://static.crates.io/crates/<name>/<name>-<version>.crate`; its SHA-256 equals both the current App lock checksum and the freshly fetched crates.io sparse-index checksum. Every regular file in each official archive was byte-compared with the task's extracted Cargo source: 10/10 files for anstyle-query, 8/8 for idna_adapter, and 16/16 for instability, with zero mismatches. Each cached source has only the additional Cargo-generated `.cargo-ok`. The independent archives and per-file identity results are retained here in `identity.json` and `identity.py`.

| Package | Archive SHA-256 | Direct parents in App all-features graph |
| --- | --- | --- |
| anstyle-query 1.1.5 | `40c48f72fd53cd289104fc64099abca73db4166ad86ea0b4341abe65af83dadc` | anstream 0.6.21 |
| idna_adapter 1.1.0 | `279259b0ac81c89d11c290495fdcfa96ea3643b7df311c138b6fe8ca5237f0f8` | idna 1.1.0 |
| instability 0.3.1 | `c8a8b1fa57a8c4b31ef130ac67b3e11b354c09e3a0453a24cade62756d52a6ee` | ratatui 0.30.2, ratatui-crossterm 0.1.2, ratatui-termina 0.1.0, ratatui-termion 0.1.2, ratatui-widgets 0.3.2 |

Read every Rust source in the archives, the normalized and original manifests, and the packaged README for each; also read the anstyle example. `files-read.json` records their hashes. Confirmed the manifests and archive inventories expose no build script or packaged binary; instability is a proc-macro. The packaged GitHub workflows are not Cargo build scripts and were not assessed as an independently run CI configuration. Dependency implementations are not completely audited here. Selected idna consumer/provider context was matched to its own lock, index and archive bytes as recorded in `context-identity.json`.

## anstyle-query 1.1.5 — agree with package-source proposal

`src/lib.rs` reads named process environment variables and derives color/terminal/CI hints. `clicolor_force()` and `no_color()` test only non-emptiness, so the literal string `0` also evaluates to true for those two functions; `clicolor()` separately treats `0` as false. These are hints, not terminal-output sanitization or precedence enforcement.

The only local `unsafe` is the Windows console API call path in `src/windows.rs:12-33`: it casts the raw stdout/stderr handles, rejects null, initializes the `GetConsoleMode` output, handles its failure, then ORs the VT flag and checks `SetConsoleMode`. The calls receive an OS handle rather than a Rust-dereferenced pointer. `enable_virtual_terminal_processing` mutates stdout first and, if different, stderr; a later failure leaves a successful stdout change in place. The example explicitly invokes this helper. Other platforms return `None`. No implicit process, file, or network operation was found in this crate. OS handle lifetimes, Windows behavior, and `windows-sys` remain separate boundaries; no Windows runtime test was performed.

I support the proposed full `safe-to-deploy` review for this exact package source, including its explicit mode-changing behavior. It is not evidence that downstream terminal text is safe or that Windows execution has been verified.

## idna_adapter 1.1.0 — agree with package-source proposal, with a correction

`src/lib.rs` is a no-std wrapper. The currently resolved `unicode-bidi 0.3.18` has 23 sequential `BidiClass` discriminants, so the local `u32` mask shifts at lines 35-37 are within range for this exact dependency version. Joining masks and character properties delegate to `idna_mapping`, `unicode-bidi`, and `unicode-normalization`. With App's `compiled_data` feature, `Adapter::new` constructs a zero-sized value. `map_normalize` uses `Mapper::new(iter, false).nfc()`; `normalize_validate` uses `true` and NFC. The inspected provider maps ignored characters to U+FFFD in the latter mode and disallowed characters to U+FFFD. The adapter itself does not perform complete domain, ASCII-deny-list, or IDNA validation and has no local unsafe or ambient I/O.

The inspected data versions differ: the idna_mapping table and unicode-bidi tables identify Unicode 16.0.0, whereas unicode-normalization identifies 17.0.0. Compatibility of the combination and full UTS #46 conformance remain open; neither a package-only audit nor the evidence here resolves them. This is not a demonstrated vulnerability.

Correction to the proposal's consumer summary: `idna 1.1.0`'s `after_punycode_decode` marks U+FFFD and differing values in a `zip` comparison, but the shown comparison does not explicitly compare lengths. It should not be described as proving rejection of *every* transformed result without additional reasoning. A limited static check of the authenticated Unicode-16 mapping table found no single mapped/deviation code point that expands with itself as the first mapped code point (`mapping-prefix-inspection.json`). This is not a multi-character normalization analysis or proof of reachability or unreachability of a length discrepancy. No affected application input or exploit is demonstrated.

On the package-source scope, I support the exact-version `safe-to-deploy` proposal with those boundaries stated; it must not be used as an IDNA conformance or combined-provider safety result. No dependency records are implied by this review.

## instability 0.3.1 — agree with package-source proposal

`src/lib.rs` parses the attribute and a supported `syn::Item`, dispatching to `src/unstable.rs`. Recognized `feature`/`issue` string arguments build documentation and a feature name; a public input is cloned and emitted as public under the named feature and `pub(crate)` otherwise. The hidden struct branch also narrows explicitly public fields. The original input code and other attributes are retained, and module contents are not recursively restricted. This is API visibility management, not a secret or authorization boundary.

Unsupported item types and non-string values for recognized attributes can panic during compilation. An unrecognized bare attribute key is silently ignored and can leave the catch-all `unstable` feature selected; this is a source-level behavior to document, not a demonstrated security issue. The proc-macro code itself contains no unsafe operation, file/network/process/environment access, or added unsafe output. `syn`, `quote`, compiler behavior, and the caller's supplied tokens remain separate. No compile or runtime test was run in this cross-review.

I support the proposed full `safe-to-deploy` review for this exact proc-macro source; its documented compile-time failure and visibility limits remain explicit.

## Boundaries

These judgments concern the exact package sources, not dependencies, a complete IDNA/domain-security proof, Windows execution, the product build, devices, or overall cargo-vet closure. No source audit record was added to the canonical registry in this cross-review. Other lane A holds were neither retried nor changed.
