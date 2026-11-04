# Lane A continuation — proposed full reviews and additional holds

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review), 2026-11-03. Three full exact-version source reviews below are **proposals for independent cross-review**, not registered records. The terminal_size, outref and string_cache_codegen findings are holds; no safety certification is proposed for them here. The graph/lock identities and criteria methodology are in `review.md` and `graph-inventory.json`; source files actually read and hashes are in `continuation-files-read.json`. Context archives and selected files were independently compared against the current lock (`continuation-context.json`). A hash establishes identity only. Dependencies, OS and deployed product are separate review subjects. No publisher trust, exemptions or wildcard records are requested.

## Proposed: anstyle-query 1.1.5

Source ID `registry+https://github.com/rust-lang/crates.io-index#anstyle-query@1.1.5`; archive SHA-256 `40c48f72fd53cd289104fc64099abca73db4166ad86ea0b4341abe65af83dadc`. Read all packaged Rust: `src/lib.rs`, `src/windows.rs`, `examples/query.rs`, plus both manifests and the README included for doctests. Archive has no build script even though `include` mentions one; Cargo's normalized manifest sets `build=false`.

The pure query functions read environment variables `CLICOLOR`, `CLICOLOR_FORCE`, `NO_COLOR`, `TERM`, `COLORTERM`, and `CI` (lib lines 23–138) and return boolean/optional capability hints; caller output and interpretation are outside this crate. The only crate-local unsafe is Windows `enable_vt` (windows lines 12–33): it obtains standard handles via the standard I/O objects, checks null, supplies an initialized local out-pointer to `GetConsoleMode`, ORs the documented VT flag and calls `SetConsoleMode`, returning OS errors. Invalid non-null handle values are sent to `GetConsoleMode` and returned as errors, not dereferenced in Rust. Stdout can be changed before a distinct stderr call fails, so the side effect is not rolled back. The explicit example calls the Windows enabling method; default unit tests examine the local helper only. Non-Windows implementation returns `None`. Windows handle stability, OS semantics and `windows-sys` are separate assumptions; no Windows execution was done. Enabling control sequences does not sanitize terminal text.

For a requested terminal-capability probe and explicitly named console mode change, propose full `safe-to-deploy`, which implies `safe-to-run`. No surprising autonomous network, file, or process activity identified. No tests, platform execution or dependency review in this pass.

```toml
[[audits.anstyle-query]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "1.1.5"
notes = "Proposed full source review: environment controls capability hints; explicit Windows helper changes standard console modes and may leave stdout changed on stderr failure. Terminal text and Windows bindings need separate review."
```

## Proposed: idna_adapter 1.1.0

Source ID `registry+https://github.com/rust-lang/crates.io-index#idna_adapter@1.1.0`; archive SHA-256 `279259b0ac81c89d11c290495fdcfa96ea3643b7df311c138b6fe8ca5237f0f8`. Read both target manifests and all 248 lines of the only Rust source; no build script or bundled native code. This proposal is a **full target review**, not dependent on the old 1.2.0 audit anchor. The 1.2.0 source and old reverse diff were also read for comparison: the target switches from ICU4X to idna_mapping, unicode-bidi and unicode-normalization, so no equivalent Unicode tables or behavior are assumed. The earlier fixed-source Mozilla 1.2.0 record is absent from the current filtered imports.lock; a bare reverse delta would need its anchor before it could satisfy cargo-vet.

The wrapper constructs masks over `unicode_bidi::BidiClass` for RTL, allowed first/last/interior classes (src/lib.rs 27–95); the exact resolved enum has 23 variants (<32) so the `u32` shift is in range. Joining masks delegate to idna_mapping (97–127); typed bidi wrappers expose mask intersection and four comparisons (130–178). `Adapter` is zero-sized; only the enabled `compiled_data` feature exposes its constructor. `is_virama`, `is_mark`, `bidi_class`, and `joining_type` delegate property queries (200–223). `map_normalize` supplies `ignored_as_errors=false`, and `normalize_validate` supplies `true`, then each applies NFC (230–246). Selected resolved provider code shows ignored input is skipped in the former and becomes U+FFFD in the latter, and disallowed inputs become U+FFFD; the idna consumer's `after_punycode_decode` rejects U+FFFD and pairwise differences in a zip comparison (idna src/uts46.rs 1492–1538), but does not explicitly compare lengths there; rejection of every transformed result is not established by that comparison. That context is not a complete consumer audit or Unicode-conformance proof.

There is no crate-local unsafe, filesystem/network/process operation, or autonomous side effect. With `idna_mapping 1.1.0` and `unicode-bidi 0.3.18` using Unicode 16 and `unicode-normalization 0.1.25` using Unicode 17 data, semantic compatibility of the actual data is a separate outstanding concern, not shown to be a vulnerability here. This proposal does not certify UTS #46 conformance, all domain-validation rules, provider tables, exact equivalence to 1.2.0, or cross-version data consistency. No conformance tests or application/network tests ran. Given the direct wrapper logic and documented separate dependency boundary, propose full `safe-to-deploy` (also `safe-to-run`) for the package source, conditional on independent review of this boundary.

```toml
[[audits.idna_adapter]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "1.1.0"
notes = "Proposed full wrapper source review. Unicode mapping, bidi and normalization providers use separately reviewed data; version compatibility, full IDNA conformance and caller validation are not established by this review."
```

## Proposed: instability 0.3.1

Source ID `registry+https://github.com/rust-lang/crates.io-index#instability@0.3.1`; archive SHA-256 `c8a8b1fa57a8c4b31ef130ac67b3e11b354c09e3a0453a24cade62756d52a6ee`. Read complete `src/lib.rs` and `src/unstable.rs`, the only Rust sources, plus both manifests. The archive has no build script or binary. Cargo invokes this procedural macro during compilation; `syn` and `quote` are independent code.

`unstable` parses attribute meta and a supported item (`lib.rs:104–121`); unsupported syntax and non-string recognized arguments can cause compile errors/panics. `UnstableAttribute::expand` clones a public item, emits the public version under the selected `cfg(feature=...)` and a `pub(crate)` version otherwise, adds doc text, and narrows public struct fields in the hidden branch (`unstable.rs:13–89,143–165`). Module contents are not recursively restricted; arbitrary non-public items pass through, matching documented intent. The generated code preserves the input item, not a secret-protection boundary. No generated unsafe, external command, network, filesystem or environment access is added by this crate; input item code and parser dependencies are separate. No compile probes ran. Propose full `safe-to-deploy` and hence `safe-to-run` for this code generator, with the input/output and compilation-failure limits noted.

```toml
[[audits.instability]]
who = "Codex AI-assisted"
criteria = "safe-to-deploy"
version = "0.3.1"
notes = "Proposed full proc-macro source review: feature-conditioned visibility is an API tool, not authorization. Unsupported items or malformed arguments can fail compilation; input code and dependencies are separately reviewed."
```

## Hold: terminal_size 0.4.4 Windows unsafe precondition

Read all Rust files and both manifests. On Unix `terminal_size_of` checks isatty and gets size through rustix; the unit test executes `stty` via PATH and requires a TTY. On Windows `src/windows.rs:17–27`, safe `terminal_size()` constructs a `BorrowedHandle` directly from `GetStdHandle` before `terminal_size_of` checks `INVALID_HANDLE_VALUE` (46–48). [Microsoft's GetStdHandle contract](https://learn.microsoft.com/en-us/windows/console/getstdhandle) states that failure returns INVALID_HANDLE_VALUE and that process-table values are not validated. Rust 1.94 `std/src/os/windows/io/handle.rs:132–151` (source SHA-256 `437b320050ad15ae40c5633172b7c57ad9f2df7bf44159696c5fc76f594e2411`) states `borrow_raw` requires a valid open resource, explaining that -1 can also represent a *valid* pseudo-handle and NULL can be associated with detached consoles. A GetStdHandle failure is not such a valid handle. Thus the code appears to violate that unsafe API's documented precondition before later filtering. This is a contract-level source finding, not a demonstrated Rust memory-UB or exploitable product path. Standard handles can also be invalidated by external handle manipulation; no Windows execution was performed. Prior notes emphasizing -1 representation did not address this ordering. Hold deploy proposal pending platform/IO-safety review or repair.

## Hold: outref 0.5.2 incomplete unsafe-constructor contract

Read complete `src/lib.rs` and normalized manifest. Safe constructors derive pointers from exclusive `&mut` and `MaybeUninit` references; reborrow shortens the lifetime and `assume_init` is unsafe. `Out::new` calls `NonNull::new_unchecked` (86–91) and must support subsequent unique mutable references for the inferred lifetime, yet its safety text names only writable validity and alignment. It does not explicitly enumerate non-nullness (including zero-sized values), exclusive access and duration of the resulting lifetime. No safe-code exploit or product call violating the constructor contract has been shown. Hold while the unsafe API contract and real callers are independently assessed; do not infer that assumptions missing from documentation are guaranteed by safe inputs to this unsafe function.

## Hold: string_cache_codegen 0.5.4 corrupts literals in generated code

Read full `lib.rs` and both manifests. `write_to` (lib.rs:168–181) stringifies the entire generated token stream and applies textual replacements, including `.replace("; ", ";\n")`. These replacements also operate **inside string literals**. Static atoms and their hashes are computed before textual replacement (204–232); inline values are packed from original bytes (268–285). The resulting generated macro matchers and static atom table can therefore disagree with the actual inputs and precomputed values.

A local fixture in `string-codegen-probe/` builds archive-matched 0.5.4 as a path dependency, plus `string_cache 0.8.9`. All 25 resolved registry packages match the current App Cargo.lock in name, version, source and checksum; its own lock SHA-256 is `8999489531e34239dc03b381ca5220a3aed1cabf9b0f0227780a49fd88d68d0a`. `cargo run --bin string-cache-codegen-literal-probe --locked --offline` using rustc 1.94 succeeded and asserted: a literal macro input containing `a;` followed by an actual newline yields the atom `a; b`; the constant generated for `long; string` yields `long;` followed by newline and `string`. `cargo check --bin original --locked --offline` failed as expected: `my_atom!("a; b")` has no matching generated rule. The fixture is a bounded local codegen demonstration; no untrusted network or service access was involved.

The current graph identifies only `markup5ever 0.12.1` as direct caller. Its archive-authenticated `build.rs` builds atom lists from `local_names.txt`, lowercase names and a fixed `NAMESPACES` array; none of those source files contains the substring `; `. Therefore the demonstrated input exists in the public generator API, but **no current App data path or security exploit is demonstrated**. Audit of other replace patterns and generated consumer semantics remains open. Hold the previous safe-to-deploy proposal until the generator bug and reasonable-use criterion are independently assessed.

## Root registration decision

Codex AI-assisted root, 2026-11-03: the three exact package-source proposals were independently cross-reviewed, including correction of the IDNA consumer summary above. Register only these package-local conclusions. Unicode provider compatibility and full domain validation are separate uncompleted review obligations; this is not IDNA conformance or combined-provider approval. All named holds remain unregistered.
