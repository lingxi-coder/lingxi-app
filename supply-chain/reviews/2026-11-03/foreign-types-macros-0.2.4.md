# foreign-types-macros 0.2.4: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed delta; baseline missing from current imports.lock**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `ea5190182e6915eb873ddbc16e23b711b6eb1f9c00a0d0a3a91b5f6228475225`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `772cb986f3568a5d6be6c88fa0f138b5e3f045d3`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"2d5077646c441fb089662e9ac439bec45c0d84b5"},"path_in_vcs":"foreign-types-macros"} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `std`. Direct reverse edges: foreign-types 0.5.0 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

Baseline 0.2.3 archive SHA-256 `1a5c6c585bc94aaf2c7b51dd4c2ba22680844aba4c687be581871a6f518c5742`, packaged tree `1b640d69d761a184d2f9f0dc1870e9d6cb639acc`; its cached crates.io index record was independently checked against the archive checksum. Third-party baseline path (checked against immutable source copies; original reviewer claims are not re-created as our own):

- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.2.3`, reviewer `Teodor Tanasoaia <ttanasoaia@mozilla.com>`, criterion `safe-to-deploy`.

These baseline records are absent from the current imports.lock; the delta cannot close the locked cargo-vet path unless the existing configured import is legitimately refreshed and rechecked. This review does not add trust or imports.

## Independently examined implementation

The 0.2.3 -> 0.2.4 packaged Rust modules are unchanged, while the manifest switches syn 2 to 3; this is not a Cargo build script (src/build.rs is a module). Parser requires caller-authored unsafe type declarations and limits asserted auto traits to Send/Sync (src/parse.rs:44-99). Generator emits NonNull owner and unsafe ForeignType/Ref, Drop, Deref, optional Clone and ToOwned (src/build.rs:45-260): nonnull valid owned pointer, correct clone/drop and thread-safety assertions are caller/native contracts. I compared current syn 2.0.117/3.0.3 parse.rs and attr.rs (byte-identical), and selected split_for_impl, push_value and push_punct bodies; this does not establish all parsing/printing equivalence or certify syn. No native calls execute by this proc macro itself. Baseline/import and separately reviewed syn/foreign-types/native code remain necessary.

Target files examined (SHA-256):

- `Cargo.toml`: `e45dfc4828ee07ba45dd428659c56a0eb57481893e18402297149de1bf17c297`
- `src/lib.rs`: `7bb43075bede4947ee8f07a05e4d1b00e0daedadbde15f1b386f97514853ad1c`
- `src/parse.rs`: `d8cc189b7108602dee81ff5436a9480dd45397101f7faad875027441e1477ef1`
- `src/build.rs`: `bee8027592dff9052e1235a6b1bfd0ae715389265247131c3e1ed865e561ba40`

The baseline Rust/manifest code and target changes described above were compared; base archive identity is above.

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
