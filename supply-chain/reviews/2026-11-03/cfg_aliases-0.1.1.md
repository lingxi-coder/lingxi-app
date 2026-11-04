# cfg_aliases 0.1.1: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `fd16c4719339c4530435d38e511904438d07cce7950afa3718a84ac36c10e89e`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `cc874a5c9cc66b38e9d5a71b0ce366045ab1a495`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"9d8077f013504d8e461923047134be556772fdb3"}} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: nix 0.28.0 (build, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

The exported cfg_aliases! macro reads CARGO_CFG_* / CARGO_FEATURE_* for the supplied identifiers and expressions, resolves all/any/not and prior aliases, and emits a cargo:rustc-cfg line when true (src/lib.rs:175-371). Output alias is an identifier, not an arbitrary newline-bearing string. Missing environment values are false except a missing key compared with an empty value may match an empty split. No local unsafe, FFI, process or network action. Malformed or cyclic caller definitions may cause compile errors. The macro only tests presence for debug_assertions, which may be absent even in a debug build. In current metadata its sole direct parent nix 0.28.0 build.rs defines target OS groups, not debug_assertions; this is a selected caller observation, not an all-build proof.

Target files examined (SHA-256):

- `Cargo.toml`: `984209d6fcb6276b505fe703019ed83d57b33accbd6416c2f86826baadb2dc1a`
- `src/lib.rs`: `83d97f5e7643893132c290cfa7754579535f6a1576ec0b4ecfea6e8759cfede9`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
