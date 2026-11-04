# by_address 1.2.1: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `64fa3c856b712db6612c019f14756e64e4bcea13337a6b33b696333a9eaa2d06`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `33106a105b005497f893ce6ea5f0724014ac533b`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"bb921a29ea2fc1ee30a4dac9ff6d97c6730b5c7f"},"path_in_vcs":""} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: palette_derive 0.7.7 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

ByAddress and ByThinAddress are repr(transparent) wrappers. from_ref casts a shared &T to the transparent wrapper with the same borrow and metadata (src/lib.rs:96-116,254-275); pointer comparisons/hashes do not dereference raw pointers. ByAddress uses full pointer including metadata; ByThinAddress intentionally discards metadata (:156-195,298-336), so differently sized slices and some trait objects may compare equal. Zero-sized objects and trait-object vtables do not guarantee stable object identity; a custom Deref can change its target and invalidate key assumptions. Debug prints the wrapped Debug value and pointer address (:120-145). No build script, normal dependencies, process, FFI or network code.

Target files examined (SHA-256):

- `Cargo.toml`: `482a78ea0af555ed73bd12e935fcf443ccce89732fa9078d0750adea9c318418`
- `src/lib.rs`: `ce07f235e47afb33632404a645b198cbf4f7036e1db116b96300c09a09a7b7ca`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
