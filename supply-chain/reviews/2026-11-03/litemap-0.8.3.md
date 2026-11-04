# litemap 0.8.3: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed delta; baseline missing from current imports.lock**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `47d9d19d1d6efa0109d2f65ff4c85cddd50bd572e5a00127ab10987290bcefae`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `c46367f26a8018d6cfcddc6e28ed2949576da64a`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"a14f2dad852be26bad277ae704abf27a15cbfed1"},"path_in_vcs":"utils/litemap"} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `alloc`. Direct reverse edges: icu_locale_core 2.3.0 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

Baseline 0.8.0 archive SHA-256 `241eaef5fd12c88705a01fc1066c48c4b36e0dd4377dcdc7ec3942cea7a69956`, packaged tree `dc93778ff3c27c0d5ff3ef7cbfdc9ff2483d6d4e`; its cached crates.io index record was independently checked against the archive checksum. Third-party baseline path (checked against immutable source copies; original reviewer claims are not re-created as our own):

- [google fixed source](https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml) at `8cec794c38751495cf7a6c083220c207af1303d6`, source SHA-256 `eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd`: `0.7.4`, reviewer `Manish Goregaokar <manishearth@google.com>`, criterion `safe-to-deploy`.
- [google fixed source](https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml) at `8cec794c38751495cf7a6c083220c207af1303d6`, source SHA-256 `eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd`: `0.7.4 -> 0.7.5`, reviewer `Daniel Cheng <dcheng@chromium.org>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.7.5 -> 0.8.0`, reviewer `Makoto Kato <m_kato@ga2.so-net.ne.jp>`, criterion `safe-to-deploy`.

These baseline records are absent from the current imports.lock; the delta cannot close the locked cargo-vet path unless the existing configured import is legitimately refreshed and rechecked. This review does not add trust or imports.

## Independently examined implementation

Reviewed source/manifest delta from 0.8.0. iter/iter_mut expose the backing store iterators, while new KeysIterWrap/ValuesIterWrap delegate next/nth/fold/next_back and size_hint (map.rs:899-942,1224-1304); ExactSize/Fused are only implemented when the inner iterator has those traits. Optional serde imports serde_core and implements 128-bit serialization methods unconditionally; current graph features show alloc only, not serde/databake/yoke. Existing Vec store sorts stably, deduplicates by retaining the last equal key, and uses safe swaps (store/vec_impl.rs:110-156,169-251). Caller-provided sorted stores must actually be sorted; untrusted input size can drive allocation. No newly introduced unsafe, build script, process or network action. This delta does not certify external stores or separately gated dependencies.

Target files examined (SHA-256):

- `Cargo.toml`: `0cc7f82691b669873f759afd43bc2c9387088bfb973ad6037decce4d94f31fe3`
- `src/lib.rs`: `deee2ddbc976fd70bbd0d415f58077139900d293eece5e162d286fc4d3295eda`
- `src/map.rs`: `7576bc0ab8eda44061986a2ecfdd58fa9dbf75b540cfe8e881bc91a7798c9dc1`
- `src/serde.rs`: `091d72257d2044ba3989a75862f0ca54ae67cbb025ff10651c364a608d7a0539`
- `src/serde_helpers.rs`: `518441e8d56778187b6652519e5a63e2f1a331c8a6b1274ffcbd931d8c0defa2`
- `src/store/mod.rs`: `ac1d6dc46ee539c201c8c4fce1f157bc9d4e68b077dd146db94b2816283e5299`
- `src/store/vec_impl.rs`: `eb08f3d0a54e2fad43d03dc570ee4db422302874f4e3e09c91d00d652b0d6a63`

The baseline Rust/manifest code and target changes described above were compared; base archive identity is above.

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
