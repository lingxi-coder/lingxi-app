# utf8_iter 1.0.4 — full exact-version source review proposal

Reviewer: Codex AI-assisted, 2026-11-03. This is a proposal for independent cross-review, not a canonical cargo-vet record. It does not use a publisher wildcard, exemption, or a previous version's audit as an anchor.

## Identity and scope

- Current App HEAD at identity verification: `5fd98978ba4ba38d8073e6609f94eb44a75bd831`; `Cargo.lock` SHA-256 `221db470e0c34f2842b439ce0d4b2530f2093e5292871de6dde1d2f1af528b52`.
- Exact lock identity: `registry+https://github.com/rust-lang/crates.io-index#utf8_iter@1.0.4`, checksum `b6c140620e7ffbb22c2dee59cafe6084a59b5ffc27a8859a5f0d494b5d52b6be`.
- Independently downloaded the [official crates.io archive](https://static.crates.io/crates/utf8_iter/utf8_iter-1.0.4.crate); SHA-256 equals the App lock, the freshly fetched crates.io index, and the local cached archive. All 11 archive files match the local Cargo source byte-for-byte; only `.cargo-ok` is extra in the cache. `identity.py` and `identity.json` record every path and hash.
- Read the complete packaged Rust sources `src/lib.rs`, `src/indices.rs`, and `src/report.rs`, both manifests, and README. The archive contains no `build.rs`, binary, proc-macro, native source, Cargo dependency, dev dependency, or custom feature. Its two packaged unit tests are in `src/report.rs`; the three doctests are in the Rust documentation. There is no external execution in these tests.
- App's all-features metadata has direct references from `icu_collator 2.3.1`, `icu_collections 2.3.0`, `icu_normalizer 2.3.0`, and `idna 1.1.0`; the precise source IDs are in `identity.json`. Those consumers and Unicode policy are outside this package review.

## Behavior and side effects

`Utf8Chars` decodes borrowed bytes into Unicode scalar values, substituting U+FFFD for malformed UTF-8 according to the documented lossy behavior. `ErrorReportingUtf8Chars` follows the same branches but returns `Err` for malformed input, preserving `Ok('\u{FFFD}')` for a real encoded replacement character. Both support double-ended iteration and expose their unconsumed input. `Utf8CharIndices` wraps the first iterator with an original-byte offset; forward movement increases the front offset and backward movement uses the remaining length.

An independent reviewer identified a documentation mismatch in `indices.rs:97-98`: it says `offset()` is the underlying string's length once exhausted, but consuming `b"a"` entirely with `next_back()` leaves the front offset at 0. The implementation's value is the number of bytes consumed from the front, including when back iteration empties the slice; the bounded reference probe checks that actual behavior. This is a public-API documentation/semantics limitation, not a demonstrated memory-safety issue or observed consumer failure.

The library is `no_std`, contains no heap allocation, ambient environment read, filesystem/network/process access, or executable build step. Error formatting writes only a fixed diagnostic to the caller's formatter; derived `Debug` for the iterators can expose remaining input bytes if a caller logs them. The iterator advances by at least one byte whenever it yields; the reverse scan inspects at most four bytes, so iteration is bounded and linear in input length. Each iterator retains an immutable subslice of the caller's original borrowed data.

## Input bounds and unsafe scalar construction

All ten unsafe operations are `char::from_u32_unchecked`: `lib.rs:134,150,189,206,222` and the corresponding duplicated branches in `report.rs:96,112,145,162,178`. No unchecked pointer or slice access exists.

- The fast path first checks `remaining.len() >= 4` before reading offsets 0 through 3. Table indexes are `second` in 0..255 and `first + 0x80` in 0..383; the table has 384 entries. ASCII and two-byte branches precede the table branches.
- A two-byte scalar requires first byte `C2..DF` and second `80..BF`; the result is U+0080..U+07FF. Fallback verifies input length before reading the second byte and uses the same ranges.
- The three-byte branch accepts `E0 A0..BF`, `E1..EC 80..BF`, `ED 80..9F`, or `EE..EF 80..BF`, followed by `80..BF`. These rules exclude overlong encodings and UTF-16 surrogates. Fallback checks length before reading the third byte and applies the same restrictions.
- The four-byte branch accepts `F0 90..BF`, `F1..F3 80..BF`, or `F4 80..8F`, followed by two `80..BF` bytes. The computed result is U+10000..U+10FFFF. The fallback consumes a validated incomplete/invalid-fourth prefix and emits an error; a valid four-byte sequence is handled by the fast path.
- `next_back` starts only when the remaining slice is nonempty, walks back no more than four candidate bytes, and splits at a position bounded by the number actually examined. It accepts a candidate only when a forward decode consumes that suffix; otherwise it removes one trailing byte as an error. The index wrapper's offsets remain bounded by the original slice length, including when front and back are interleaved.

`classification_check.py` compares the actual 384-byte table's fast-branch decisions against the independent scalar UTF-8 lead/continuation ranges above, over all first/second bytes and third/fourth high-bit classes: 83,968 three-byte and 65,536 four-byte decisions all matched (`classification-result.json`). The low six bits of the third and fourth bytes do not affect the table decision; accepted extrema were checked for scalar range. This supplements the source reasoning rather than replacing it.

## Bounded independent-reference checks

On aarch64 macOS with `rustc 1.94.0 (4a4ef493e 2026-03-02)`, compiled the authenticated source directly as an rlib and ran `reference_probe.rs` (`reference-probe-result.json`). It compared Rust's `String::from_utf8_lossy`, `std::str::from_utf8` error boundaries, and `char::encode_utf8` against the package:

- 1,189,905 byte sequences: empty, every one- and two-byte input, every first/second byte combined with each third/fourth high-bit class, padded short sequences to reach the fast path, and 10,000 deterministic longer samples.
- All 1,112,064 Unicode scalar values encoded by the standard library.
- Forward, reverse and alternating front/back iteration, exact error-versus-real-U+FFFD reporting, byte indices, `last`, `count`, exhaustion, `offset` and the remaining slice were checked where applicable. All assertions passed.

The source's own tests also passed: `rustc +1.94.0 --edition=2021 --test <authenticated-src>/lib.rs` followed by the resulting binary ran 2/2; `rustdoc +1.94.0 --edition=2021 --test <authenticated-src>/lib.rs --extern utf8_iter=<review-rlib>` ran 3/3 doctests. The initial local compilation used the directory's default Rust 1.98.1, but it was replaced by the explicitly selected 1.94.0 build before any probe or upstream test was run.

## Judgment and limitations

I propose a full `safe-to-deploy` review for exactly `utf8_iter 1.0.4` (which also satisfies `safe-to-run`), based on the full package source, unsafe preconditions, identity evidence, and bounded checks above. Suggested attribution is `Codex AI-assisted`. The checks are finite and host-only, not formal proof, exhaustive enumeration of every long input, a memory-model proof via Miri, or a review of the consuming ICU/IDNA packages. No canonical audit record was written; registration requires an independent substantive cross-review of these conclusions.
