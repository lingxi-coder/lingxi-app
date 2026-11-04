# time-core 0.1.9: independent source cross-review

Reviewer: Codex AI-assisted. Status: **Proposed delta; baseline missing from current imports.lock**. Criterion assessed: `safe-to-deploy`.

## Identity and graph

Registry source: crates.io (`registry+https://github.com/rust-lang/crates.io-index`); exact package archive SHA-256 `9e1c906769ad99c88eaa54e728060edef082f8e358ff32030cb7c7d315e81109`, equal to the integrated App Cargo.lock checksum. Git tree SHA-1 of all packaged members, preserving executable modes: `092dd0d3377e72328466c309b26801c286e9297a`. This is the computed package tree, not a claimed upstream Git tree. Packaged `.cargo_vcs_info.json`: `{"git":{"sha1":"5d8737c39b9170c36c99cfc1f9f49cf0c059c63a"},"path_in_vcs":"time-core"} (declared by package; upstream commit tree not independently authenticated)`.

Current source directory was compared byte-for-byte with every archive member and no other files beyond Cargo cache markers were present. Current metadata union features: `(none)`. Direct reverse edges: time 0.3.55 (normal, all targets), time-macros 0.2.32 (normal, all targets). Union graph edges and features do not prove a single target builds or runs every edge.

Integrated App HEAD `8febcb390244e41cfcf761a1b90632077af2eb97` (tree `a85e257397d20c096c6aa765657a383bfc4e72f7`); Cargo.lock SHA-256 `0d56f11414e9534315c6bd92e4e1430692b43ee65cc29c28bd9b213b140d57e5`; metadata SHA-256 `13d48bdffa5a21c1724334b35e5cc52bb6870e4671c7601ad3933b0fd538851a` (1008 packages). The formal audit file had 31 entries and no entry for this package when checked. This package's dependencies have independent review obligations.

Baseline 0.1.8 archive SHA-256 `7694e1cfe791f8d31026952abf09c69ca6f6fa4e1a1229e18988f06a04a12dca`, packaged tree `5e62303fd193bd3bc0a36fd76df812bdabf12e86`; its cached crates.io index record was independently checked against the archive checksum. Third-party baseline path (checked against immutable source copies; original reviewer claims are not re-created as our own):

- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.1.0`, reviewer `Kershaw Chang <kershaw@mozilla.com>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.1.0 -> 0.1.1`, reviewer `Kershaw Chang <kershaw@mozilla.com>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.1.1 -> 0.1.2`, reviewer `Alex Franchuk <afranchuk@mozilla.com>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.1.2 -> 0.1.4`, reviewer `Lars Eggert <lars@eggert.org>`, criterion `safe-to-deploy`.
- [mozilla fixed source](https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml) at `e6648794a1deb2821ddebd846564fd99f420b432`, source SHA-256 `97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d`: `0.1.4 -> 0.1.8`, reviewer `Lars Eggert <lars@eggert.org>`, criterion `safe-to-deploy`.

These baseline records are absent from the current imports.lock; the delta cannot close the locked cargo-vet path unless the existing configured import is legitimately refreshed and rechecked. This review does not add trust or imports.

## Independently examined implementation

Compared 0.1.8 src/convert.rs to 0.1.9 src/unit.rs and both manifests. New zero-sized const-N unit types and Unit enum offer comparisons by unit kind (unit.rs:21-28,90-160,256-311); notably Unit<0> of different kinds still compares different. per/per_t return compile-time constants (unit.rs:30-88); independently compared the complete impl_per invocation after removing the old per-type display strings and whitespace: normalized tables are identical, SHA-256 51c8472052a72837d849a6e733b2519753be875ccfab581f2116e1639296f868. The old public convert module is replaced with unit, so this is not a compatibility assertion. No newly introduced unsafe, IO, subprocess or native code. Baseline and downstream time crates remain separate review obligations.

Target files examined (SHA-256):

- `Cargo.toml`: `e4fcb8945ac8299fa6ece980320f557d1f1b32f9281e4dc9d047c87bc81fb41e`
- `src/lib.rs`: `471d4a128ebd5fec6565c63c1e546bdba7154d7c434e9323bbe5343c3b8ad3ed`
- `src/unit.rs`: `5149bc68ad39ac3c1a36b2b2c09d2bf699a90e52284a49b4eec53cb65e5fc4bb`

The baseline Rust/manifest code and target changes described above were compared; base archive identity is above.

## Boundary of conclusion

This is static, package-local AI-assisted source review of the described version or delta. It does not assert formal proof, all-feature or all-platform execution, independent approval of dependencies, whole-product safety, or completion of cargo-vet/CI. The findings and input/output conditions above are part of the conclusion. No trust, exemption, wildcard, Cargo pin, or formal audit store was changed by this review.
