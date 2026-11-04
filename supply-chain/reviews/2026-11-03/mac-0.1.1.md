# mac 0.1.1: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `c41e0c4fef86961ac6d6f8a82609f55f31b05e4fce149ac5710e439df7619ba4`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `33f17d42b0100accd71f0542c26012fe5afa78f8`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `not shipped in archive`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: futf 0.1.5 (normal, all targets), html5ever 0.27.0 (normal, all targets), tendril 0.4.3 (normal, all targets), xml5ever 0.18.1 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

The package exports macro_rules helpers, with no normal dependency or build script. match_cfg emits cfg-gated match arms; overlapping conditions can give unreachable-pattern diagnostics and its fallback-free form uses an obsolete static_assert attribute. format_if evaluates formatting only on its chosen branch; inspect! prints the selected value and source location, potentially exposing sensitive values if the caller chooses them. addrs_of! uses unsafe transmute of references to integer addresses and repeats the caller's object expression for each field (src/mem.rs:33-43); it does not dereference these integers. Equal field addresses alone do not establish the validity of the caller-authored transmutation illustrated in its documentation. No hidden build-time network/process action; all macro inputs and effects are caller-selected.

Target files examined (SHA-256):

- `Cargo.toml`: `6d465c5e5a4b3e197049e9fb7393b2f996c1c654e6ded700afce046fac2f7969`
- `src/lib.rs`: `f9c7705196f71dcabb4cfe1636f1b71b6362abb7eac70ddaa6e05cf9a0683dee`
- `src/cfg.rs`: `63e9c4da8b0f588c6af1e157e626044fc1fb52757348aa466d08d69795335f33`
- `src/format.rs`: `3c528949cdde598d3ddc5abea31b136ca256c7bb3932e053c5ff6e49775579a3`
- `src/inspect.rs`: `cf039cc721daa177b13d6b432589ceef3bdfa6078af3f71c34c28d2eca68d0c3`
- `src/matches.rs`: `1ef2b80946cc5e3cd15e20a22f62b832066d2e2445881191741a83abe80e7566`
- `src/mem.rs`: `f74d9172bb136b370c33fa524aae3543cf37815eeacf1d862b29b3c8c32a6aee`
- `src/syntax_ext.rs`: `5c938b44d7aaa0f47eef495f4a42f1f91cd11fb0d1b20915786085b77e1db284`
- `src/test.rs`: `895fc0aed7f5f3863025b85de4d01c2551bed51778ca8846920be7febff837ac`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
