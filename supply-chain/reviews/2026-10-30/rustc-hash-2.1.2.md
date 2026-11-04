# Source review: rustc-hash 2.1.1 -> 2.1.2

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "2.1.2",
    "archive_sha256": "94300abf3f1ae2e2b8ffb7b58043de3d399c73fa6f4b73826402a5c457614dbe",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f5d5b78a7e13e84e07b0f7e2a1115922ab341cd594a53983bd413aca74b6316a",
        "index_record_sha256": "f86b5183ff6926f3a079a9828d8fd251de2549a663f14153b43cc7aa5a5c9939",
        "cksum": "94300abf3f1ae2e2b8ffb7b58043de3d399c73fa6f4b73826402a5c457614dbe",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f5d5b78a7e13e84e07b0f7e2a1115922ab341cd594a53983bd413aca74b6316a",
        "index_record_sha256": "f86b5183ff6926f3a079a9828d8fd251de2549a663f14153b43cc7aa5a5c9939",
        "cksum": "94300abf3f1ae2e2b8ffb7b58043de3d399c73fa6f4b73826402a5c457614dbe",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "2.1.1",
    "archive_sha256": "357703d41365b4b27c590e3ed91eabb1b663f07c4c084095e60cbed4362dff0d",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f5d5b78a7e13e84e07b0f7e2a1115922ab341cd594a53983bd413aca74b6316a",
        "index_record_sha256": "74cdf169bc0a59ccf879feda3edbe4a587e6878e90d6c67e80b1705a8bced961",
        "cksum": "357703d41365b4b27c590e3ed91eabb1b663f07c4c084095e60cbed4362dff0d",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f5d5b78a7e13e84e07b0f7e2a1115922ab341cd594a53983bd413aca74b6316a",
        "index_record_sha256": "74cdf169bc0a59ccf879feda3edbe4a587e6878e90d6c67e80b1705a8bced961",
        "cksum": "357703d41365b4b27c590e3ed91eabb1b663f07c4c084095e60cbed4362dff0d",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Bobby Holley <bobbyholley@gmail.com>",
        "criteria": "safe-to-deploy",
        "version": "1.1.0",
        "notes": "Straightforward crate with no unsafe code, does what it says on the tin.",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Ben Dean-Kawamura <bdk@mozilla.com>",
        "criteria": "safe-to-deploy",
        "delta": "1.1.0 -> 2.1.1",
        "notes": "Simple hashing crate, no unsafe code.",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "afbb3c737b1463a6ef5ba68383db3ddcd9cc1228ea88b9f4fa488619f7bb2fb2"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "18714b27c324e1100a99a311c0104619f4c000aa44070928d72c8cd3e3ec8090"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "CHANGELOG.md",
      "sha256": "1c9951d52d63dfbff8d32ad7909761517db5dc8f9084dba7892da8d5028c9692"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "CHANGELOG.md",
      "sha256": "99bb0a76f5f4d89142295597be863b253a74276955ea9c669c49eef05828284a"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "213c05814f6402c09bc5bf32579187b5448ec81f8530f58d7289fd50a35d5b2a"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "a121372b7ec1431a3bce191d29046a17c49102fd3f5540e959df19ffab4ddb0b"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "110004d0c56ebe79a83b2cbe44b1574a69010a9e4a8581e5215ac14251ef30cc"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "2db71897d3dc8cbd86f28507c3929a37b1d0a1e436037e40f935b647ee51e802"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "4c6ecfcf11005839dc797e6de41b70c97ab043a9a9fe8cc1c27b904252ce8ae7"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "921bad999d97d293ab4a01163dfb9add86f20b4e1716cb32596da3795b8bcdb6"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "6928d71e403482e0e6f3324fbcef23a731c9236a5315db829f4020991064c5fa"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "59a4b66a26275ca202f834cb7bc2feda135136533f0e8599af97d2563d15f72f"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "18714b27c324e1100a99a311c0104619f4c000aa44070928d72c8cd3e3ec8090",
    ".github/workflows/rust.yml": "ff0288f89c97203b725441e9d9717fa0c049f1cebb2bcbb556981bfa8be10029",
    ".gitignore": "59bcdcff558f4f15177734ef9bf5dc3cbe0c7701c0de3d302056b92888daeb8d",
    "CHANGELOG.md": "99bb0a76f5f4d89142295597be863b253a74276955ea9c669c49eef05828284a",
    "CODE_OF_CONDUCT.md": "3e77f5476805b69467641b2c682aa2355344395056939089182cd901c56dce63",
    "Cargo.lock": "a121372b7ec1431a3bce191d29046a17c49102fd3f5540e959df19ffab4ddb0b",
    "Cargo.toml": "2db71897d3dc8cbd86f28507c3929a37b1d0a1e436037e40f935b647ee51e802",
    "Cargo.toml.orig": "921bad999d97d293ab4a01163dfb9add86f20b4e1716cb32596da3795b8bcdb6",
    "LICENSE-APACHE": "95bd3988beee069fa2848f648dab43cc6e0b2add2ad6bcb17360caf749802bcc",
    "LICENSE-MIT": "30fefc3a7d6a0041541858293bcbea2dde4caa4c0a5802f996a7f7e8c0085652",
    "README.md": "ccd7a15a2e2021dbbfd5b7f99a10666a64ac50f8d5d6926a858efdde724fb424",
    "src/lib.rs": "59a4b66a26275ca202f834cb7bc2feda135136533f0e8599af97d2563d15f72f",
    "src/random_state.rs": "39063b702c38dc93b7a9039f19f4acfdc539acf1604584a87eeb43cca149ca7e",
    "src/seeded_state.rs": "530ba6e25d766231cc7540f968d3e41c5af5a38d936542b407010b9d35746fd8"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "afbb3c737b1463a6ef5ba68383db3ddcd9cc1228ea88b9f4fa488619f7bb2fb2",
    ".github/workflows/rust.yml": "ff0288f89c97203b725441e9d9717fa0c049f1cebb2bcbb556981bfa8be10029",
    ".gitignore": "59bcdcff558f4f15177734ef9bf5dc3cbe0c7701c0de3d302056b92888daeb8d",
    "CHANGELOG.md": "1c9951d52d63dfbff8d32ad7909761517db5dc8f9084dba7892da8d5028c9692",
    "CODE_OF_CONDUCT.md": "3e77f5476805b69467641b2c682aa2355344395056939089182cd901c56dce63",
    "Cargo.lock": "213c05814f6402c09bc5bf32579187b5448ec81f8530f58d7289fd50a35d5b2a",
    "Cargo.toml": "110004d0c56ebe79a83b2cbe44b1574a69010a9e4a8581e5215ac14251ef30cc",
    "Cargo.toml.orig": "4c6ecfcf11005839dc797e6de41b70c97ab043a9a9fe8cc1c27b904252ce8ae7",
    "LICENSE-APACHE": "95bd3988beee069fa2848f648dab43cc6e0b2add2ad6bcb17360caf749802bcc",
    "LICENSE-MIT": "30fefc3a7d6a0041541858293bcbea2dde4caa4c0a5802f996a7f7e8c0085652",
    "README.md": "ccd7a15a2e2021dbbfd5b7f99a10666a64ac50f8d5d6926a858efdde724fb424",
    "src/lib.rs": "6928d71e403482e0e6f3324fbcef23a731c9236a5315db829f4020991064c5fa",
    "src/random_state.rs": "39063b702c38dc93b7a9039f19f4acfdc539acf1604584a87eeb43cca149ca7e",
    "src/seeded_state.rs": "530ba6e25d766231cc7540f968d3e41c5af5a38d936542b407010b9d35746fd8"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/rustc-hash-2.1.2/target/Cargo.toml",
      "sha256": "2db71897d3dc8cbd86f28507c3929a37b1d0a1e436037e40f935b647ee51e802",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/rustc-hash-2.1.2/target/Cargo.toml.orig",
      "sha256": "921bad999d97d293ab4a01163dfb9add86f20b4e1716cb32596da3795b8bcdb6",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/rustc-hash-2.1.2/target/src/lib.rs",
      "sha256": "59a4b66a26275ca202f834cb7bc2feda135136533f0e8599af97d2563d15f72f",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/rustc-hash-2.1.2/full.diff",
      "sha256": "b96b2fb9264017639ce481800a6dbfca1e016cdb11ca574fa800dd59001b5cb1",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/rustc-hash-2.1.2/read-files.json",
      "sha256": "89aabde29ded8eb079a334f7807f24bbe605c3ba9d21e170a9e8fa67552597c6",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "b96b2fb9264017639ce481800a6dbfca1e016cdb11ca574fa800dd59001b5cb1",
  "primary_report_sha256": "1e5aeb36bed4047cb57c801f5b1c4c951cd0c65bc7562015e5bc3eb5dbba3a5e",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# rustc-hash-2.1.2: delta 2.1.1 → 2.1.2

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: mozilla:1.1.0 → mozilla:1.1.0 -> 2.1.1. `inventory.json` preserves every raw original record, source, and archive/index checksum; `preflight.json` records source commit, blob, SHA-256 and frozen lock. Each record explicitly contains safe-to-deploy and the path is contiguous.

- `src/lib.rs:17-22,93-119,343-346`: nightly-only feature/const-Default gating changes compilation capability, not the seed; stable and nightly Default both call the inherent zero-seeded constructor. `Cargo.toml:12-18,35-50` adds Rust 1.77 MSRV, unchanged default `std`, optional `rand`, nightly feature, no build script; `rand` is separate and not audited here.
- `src/lib.rs:223-274`: changes architecture selector for full u64 product versus 32-bit-decomposed mixing, and uses wrapping multiplies. Both operands fit exactly in u128 (or u32 product in u64); wrapping cannot introduce arithmetic overflow. Some architectures now use a different mixing branch, so hash outputs need not stay stable across versions/targets. `README.md:6-9` and `src/lib.rs:55-58` explicitly describe this as non-cryptographic and not intended for HashDoS resistance. Do not use as attacker-resistant hashing.
- `src/lib.rs:289-333`: for len > 16 the new split-first-chunk operates on `[..len-1]`, so it consumes exactly the offsets of old loop `off < len-16`; each chunk yields exactly two 8-byte sub-slices, and the final `len-16..` suffix is valid. Probe checks lengths 17..10000 and its adjacent algebra explains the unbounded integer case; it is not a platform runtime proof. Small input branches are unchanged.
- Complete diff includes the vcs metadata, version-only Cargo.lock change, two manifests and changelog. No new/removed binary, build script, C/assembly, permissions or system command. Static audit finds no new unsafe/FFI/concurrency path in the delta.

All changed hunks of every added/deleted/changed text file were read in `full.diff` (SHA-256 `b96b2fb9264017639ce481800a6dbfca1e016cdb11ca574fa800dd59001b5cb1`), with six lines of context; `read-files.json` lists both versions and their hashes. Unchanged files rely on the cited base. The target and base archives have no binary members. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.rustc-hash]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "2.1.1 -> 2.1.2"
```


## Independent cross-review

src/lib.rs:87-127 gates equivalent zero-seeded Default impls for nightly/stable. At :222-275 architecture selection changes the multiply mix; the u64×u64 and u32×u32 products fit their widened types. At :310-333, the chunk loop runs floor((len−1)/16) times, the same count as the old off < len−16 condition; final suffix is in range for len>16. This remains a noncryptographic hasher and is not suitable for hostile collision resistance. Manifest adds MSRV 1.77; no unsafe is added. Mozilla chain reaches 2.1.1.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
