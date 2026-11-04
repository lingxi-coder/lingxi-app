# tower-service 0.3.3: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed full source review**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `8df9b6e13f2d32c91b9bd719c00d1958837bc7dec474d94952798cc8e69eeec3`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `2a3f416f7e13008973c09e21c1253eb8eec66666`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"646804d77eebf072dac180cb5e1256b9ee7e0229"},"path_in_vcs":"tower-service"} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: hyper-rustls 0.27.5 (normal, all targets), hyper-timeout 0.5.2 (normal, all targets), hyper-util 0.1.20 (normal, all targets), lingxi-hyper-rustls 0.27.5 (normal, all targets), lingxi-hyper-util 0.1.21 (normal, all targets), lingxi-reqwest 0.12.29 (normal, cfg(not(target_arch = "wasm32"))), reqwest 0.12.28 (normal, cfg(not(target_arch = "wasm32"))), tonic 0.14.2 (normal, all targets), tower 0.5.3 (normal, all targets), tower-http 0.6.11 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

## Independently examined implementation

The only executable implementations are the forwarding Service<Request> implementations for &mut S and Box<S> (src/lib.rs:311-390); poll_ready and call pass the context/request and result directly to the same wrapped service. The trait's associated Error and Future define how results return; readiness, resource release, cancellation and any network behavior belong to implementations. Manifest has no normal dependencies, build script or binary; src/lib.rs forbids unsafe. No package-local IO, subprocess or FFI found.

Target files examined (SHA-256):

- `Cargo.toml`: `98a29ed09a717054bacd7ae7515e59000303932d04c5da5d5af0f4828f40453d`
- `src/lib.rs`: `6d278dd3dba5d17e5be2e3a0af6f181be93eede2510129a7877f8eb86bba1092`

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
