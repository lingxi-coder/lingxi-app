# Source review: new_debug_unreachable 1.0.4 -> 1.0.6

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "1.0.6",
    "archive_sha256": "650eef8c711430f1a879fdd01d4745a7deea475becfb90269c06775983bbf086",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "c6d6b2579f7062ebb66b79d0eabcaa6d6690ec5ce73c1dbedc5423fc3087e4e9",
        "index_record_sha256": "457d5a680dfaacf6cd39c87d09cffccdb6757cfd862279af75bee2aec70e4cb4",
        "cksum": "650eef8c711430f1a879fdd01d4745a7deea475becfb90269c06775983bbf086",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "c6d6b2579f7062ebb66b79d0eabcaa6d6690ec5ce73c1dbedc5423fc3087e4e9",
        "index_record_sha256": "457d5a680dfaacf6cd39c87d09cffccdb6757cfd862279af75bee2aec70e4cb4",
        "cksum": "650eef8c711430f1a879fdd01d4745a7deea475becfb90269c06775983bbf086",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "1.0.4",
    "archive_sha256": "e4a24736216ec316047a1fc4252e27dabb04218aa4a3f37c6e7ddbf1f9782b54",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "c6d6b2579f7062ebb66b79d0eabcaa6d6690ec5ce73c1dbedc5423fc3087e4e9",
        "index_record_sha256": "93400c7ac1192f20b7db84df9b98723b9dc0c692de0ba5b85dfccebce9116123",
        "cksum": "e4a24736216ec316047a1fc4252e27dabb04218aa4a3f37c6e7ddbf1f9782b54",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "c6d6b2579f7062ebb66b79d0eabcaa6d6690ec5ce73c1dbedc5423fc3087e4e9",
        "index_record_sha256": "93400c7ac1192f20b7db84df9b98723b9dc0c692de0ba5b85dfccebce9116123",
        "cksum": "e4a24736216ec316047a1fc4252e27dabb04218aa4a3f37c6e7ddbf1f9782b54",
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
        "version": "1.0.4",
        "notes": "This is a trivial crate.",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "a8ee8eb3e295421e55b98ccd903f42182cd65d2d7b692ef5d9078409cba5d671"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "6786bdb6b634c67c60a14a18d6ea40add943b29ed483bb9e2693b22e28af18d1"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "438cea17a96e11f787f827aab8c4fe2d096d75b1db95b5749339b9d2b2554d5b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "b2b94213dc095ad155858adb9342ccee8f5839b30761c77a8c2a1a90c6f28ce2"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "bcaf0900427df4f1e1f61bfaf99ec22422be14b97432ace1287e52803e9c6f0b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "c89bebeeaf164811c20eca172f592e26a1d21d8d91b3c9a367ad745015eb78e2"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "df2ae1d6bf7e507f345f17d48760aab193bc9a51de066bb1afa20d80a7a105d5"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "3ec7bb009318c1d1a148c1c1a9c12be9279ab73b72615f1edadaa90f9b6856ca"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "5a2cbd34e3a52ea1a8e1b31dd34c5ec31ce549fec30f60a0e20b87d8138da416"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "84efbf03c7f7d97e3133ad03da3b9a6941a2c1ea28134a60d82270423f9a807d"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/check.rs",
      "sha256": "323e500890599105193b6b07563a2c570ea0cc56342c81670075e5fd2ff467d1"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "tests/check.rs",
      "sha256": "85c0af86aa50de966d85c19fb88b47aec8c85532185b63cae0bef9c787baead5"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "6786bdb6b634c67c60a14a18d6ea40add943b29ed483bb9e2693b22e28af18d1",
    ".gitignore": "f58cbb29ee4ff8a030c1e32d3f4ac2b19753d7fdf8f72d050d4bda1353364fda",
    ".travis.yml": "d91d0d75087934c2d0503a8c04439fea459a19182021bb0b699644d442f5b6fc",
    "Cargo.lock": "b2b94213dc095ad155858adb9342ccee8f5839b30761c77a8c2a1a90c6f28ce2",
    "Cargo.toml": "c89bebeeaf164811c20eca172f592e26a1d21d8d91b3c9a367ad745015eb78e2",
    "Cargo.toml.orig": "3ec7bb009318c1d1a148c1c1a9c12be9279ab73b72615f1edadaa90f9b6856ca",
    "LICENSE-MIT": "f7715d38a3fa1b4ac97c5729740752505a39cb92ee83ab5b102aeb5eaa7cdea4",
    "README.md": "614d60e78bd8c3d8172b2650f86d44c703471dab8a35805736861f09158e5501",
    "examples/simple.rs": "f1fac7f8308469568b601218ea03d213584f0a51b779a3bcb78ecb40ede72483",
    "src/lib.rs": "84efbf03c7f7d97e3133ad03da3b9a6941a2c1ea28134a60d82270423f9a807d",
    "tests/check.rs": "85c0af86aa50de966d85c19fb88b47aec8c85532185b63cae0bef9c787baead5"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "a8ee8eb3e295421e55b98ccd903f42182cd65d2d7b692ef5d9078409cba5d671",
    ".gitignore": "f58cbb29ee4ff8a030c1e32d3f4ac2b19753d7fdf8f72d050d4bda1353364fda",
    ".travis.yml": "d91d0d75087934c2d0503a8c04439fea459a19182021bb0b699644d442f5b6fc",
    "Cargo.lock": "438cea17a96e11f787f827aab8c4fe2d096d75b1db95b5749339b9d2b2554d5b",
    "Cargo.toml": "bcaf0900427df4f1e1f61bfaf99ec22422be14b97432ace1287e52803e9c6f0b",
    "Cargo.toml.orig": "df2ae1d6bf7e507f345f17d48760aab193bc9a51de066bb1afa20d80a7a105d5",
    "LICENSE-MIT": "f7715d38a3fa1b4ac97c5729740752505a39cb92ee83ab5b102aeb5eaa7cdea4",
    "README.md": "614d60e78bd8c3d8172b2650f86d44c703471dab8a35805736861f09158e5501",
    "examples/simple.rs": "f1fac7f8308469568b601218ea03d213584f0a51b779a3bcb78ecb40ede72483",
    "src/lib.rs": "5a2cbd34e3a52ea1a8e1b31dd34c5ec31ce549fec30f60a0e20b87d8138da416",
    "tests/check.rs": "323e500890599105193b6b07563a2c570ea0cc56342c81670075e5fd2ff467d1"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/new_debug_unreachable-1.0.6/target/Cargo.toml",
      "sha256": "c89bebeeaf164811c20eca172f592e26a1d21d8d91b3c9a367ad745015eb78e2",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/new_debug_unreachable-1.0.6/target/Cargo.toml.orig",
      "sha256": "3ec7bb009318c1d1a148c1c1a9c12be9279ab73b72615f1edadaa90f9b6856ca",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/new_debug_unreachable-1.0.6/target/src/lib.rs",
      "sha256": "84efbf03c7f7d97e3133ad03da3b9a6941a2c1ea28134a60d82270423f9a807d",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/new_debug_unreachable-1.0.6/full.diff",
      "sha256": "1a398f98bade7f6aafcda1e71ec185abc177e1bf2d2c34e5015b2211dda3cf05",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/new_debug_unreachable-1.0.6/read-files.json",
      "sha256": "f47c18cce14a154e3fc938271c12040d35551261f6503d8434f0df9bd682e51d",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "1a398f98bade7f6aafcda1e71ec185abc177e1bf2d2c34e5015b2211dda3cf05",
  "primary_report_sha256": "18bde8bb2b1e9516170893d8189b38aece29f900a7e54601b4321beddb2ff79f",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# new_debug_unreachable-1.0.6: delta 1.0.4 → 1.0.6

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: mozilla:1.0.4. `inventory.json` preserves original records, source attribution and both archive checksums; `preflight.json` records source commits, blobs, SHA-256 and frozen lock. Every record explicitly contains safe-to-deploy and the path is contiguous.

- `src/lib.rs:28-38`: with debug assertions enabled, `$e` goes to `unreachable!` instead of `panic!`; it still panics, now with an unreachable-prefix diagnostic, and formatting/edition changes may reject previously accepted call sites. With assertions off it still calls `unreachable_unchecked`, without evaluating `$e`. Reaching this macro in optimized code is undefined behavior; it is exposed as an unsafe operation requiring a surrounding unsafe context and caller-established unreachability. This is the preexisting explicit contract, not proof callers uphold it.
- `Cargo.toml:12-36` upgrades edition 2018 to 2021, no new dependencies or build script and unchanged crate name mapping. Lock/vcs changes are metadata; tests/check.rs only removes a trailing blank line, README/examples and other resources unchanged. No C/asm/binary or system-command path introduced.

All changed hunks of every added/deleted/changed file were read in `full.diff` (SHA-256 `1a398f98bade7f6aafcda1e71ec185abc177e1bf2d2c34e5015b2211dda3cf05`), with six lines of context; `read-files.json` lists source pairs and their hashes. Unchanged files rely on the cited base. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.new_debug_unreachable]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "1.0.4 -> 1.0.6"
```


## Independent cross-review

Delta src/lib.rs:28-38 changes debug panic! to unreachable!, while the release call remains core::hint::unreachable_unchecked. The unchanged unsafe precondition means callers must prove the branch unreachable; the expansion still requires an unsafe context. Cargo.toml changes edition 2018 to 2021 and no dependencies are introduced. The independently verified Mozilla safe-to-deploy baseline is 1.0.4. This is a delta conclusion, not proof of caller invariants.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
