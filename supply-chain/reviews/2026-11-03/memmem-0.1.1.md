# memmem 0.1.1: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `a64a92489e2744ce060c349162be1c5f33c6969234104dbd99ddb5feb08b8c15`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `d9c7605b2f7afba55e9a015696269280448b6d0e`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `not shipped in archive`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: termwiz 0.23.3 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

Searcher returns an offset or None. TwoWaySearcher handles an empty needle as zero, derives critical suffix/period, checks that the complete window fits by get() before indexed comparisons, then advances by the mismatch/period or returns the first match (src/two_way.rs:39-57,134-266,282-314). No local unsafe/FFI/IO; the period arithmetic and arbitrary-length algorithm are not formally proved. Independently compiled the archive-matched source with rustc 1.98.1 (aarch64-apple-darwin) and compared 200,000 deterministic binary cases against a naive windows reference, including embedded hits, small alphabet and empty needles; maximum haystack 128 and needle 40 bytes. Probe SHA-256 bbf9e28390251e4434ed9cdeba9362bc6f43af4a94bf832b4c2f26cc917ac06c. All matched; this is bounded host evidence only.

Target files examined (SHA-256):

- `Cargo.toml`: `47f04063301db716798bdebee1b5decef8fe8aebf02b4a154f055567e6d7078c`
- `src/lib.rs`: `5dbb423ff58b8ddb4cdcb9c26d3b4baa073c74dc674a7425bf84be22e4437efb`
- `src/two_way.rs`: `4b6c5fb2a7bb26fa1abed2eb514dce66dca539eec28eb967153a15f81d4f8d0e`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
