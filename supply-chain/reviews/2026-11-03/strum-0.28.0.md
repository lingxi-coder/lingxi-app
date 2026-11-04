# strum 0.28.0: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed delta; baseline missing from current imports.lock**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `9628de9b8791db39ceda2b119bbe13134770b56c138ec1d3af810d045c04f9bd`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `1c619bb2c7b390583433b98cca21520bb4c9f784`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"7376771128834d28bb9beba5c39846cba62e71ec"},"path_in_vcs":"strum"} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `default, derive, std, strum_macros`. Direct reverse edges: jsonschema 0.58.6 (normal, all targets), ratatui-core 0.1.2 (normal, all targets), ratatui-widgets 0.3.2 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

Baseline 0.27.1 archive SHA-256 `f64def088c51c9510a8579e3c5d67c65349dcf755e5479ad3d010aa6454e2c32`, packaged tree `c77fb9301ba9b41fab76980c11dfe94fedf2d149`; its cached crates.io index record was independently checked against the archive checksum. Third-party baseline path (checked against immutable source copies; original reviewer claims are not re-created as our own):

- [google fixed source](https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml) at `8cec794c38751495cf7a6c083220c207af1303d6`, source SHA-256 `eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd`: `0.25.0`, reviewer `danakj@chromium.org`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.25.0 -> 0.26.3`, reviewer `Teodor Tanasoaia <ttanasoaia@mozilla.com>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.26.3 -> 0.27.1`, reviewer `Erich Gubler <erichdongubler@gmail.com>`, criterion `safe-to-deploy`.

These baseline records are absent from the current imports.lock; the delta cannot close the locked cargo-vet path unless the existing configured import is legitimately refreshed and rechecked. This review does not add trust or imports.

## Independently examined implementation

Compared 0.27.1 to 0.28.0 including manifests, README and source hunks. ParseError::fmt moves from std-gated to core::fmt without changing its constant message (src/lib.rs:39-54); most remaining Rust changes are documentation. Other code declares enum-related traits and optionally reexports strum_macros; derive expansion is a separately reviewed dependency. Manifest raises MSRV and selects strum_macros 0.28 and optional phf 0.13; current union features enable derive, not phf. There is no local build script, unsafe, process, network or native payload. This is a delta conclusion and does not certify those selected dependencies or API compatibility.

Target files examined (SHA-256):

- `Cargo.toml`: `1ce9b2ea1b59eb73530fe3da5d764ae45df5f0da6e518333508d41bda017e650`
- `src/lib.rs`: `018953f34f21a67b1b46220ea3e348716740c8fc33ac4bd7b69c16eebbcbd8d3`
- `src/additional_attributes.rs`: `c882921eaf123a56d6f8bd36925e857011248f7a3cb05eed0b2703478db6d1ae`

The baseline Rust/manifest code and target changes described above were compared; base archive identity is above.

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
