# Source review: askama_macros 0.16.0 -> 0.16.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.16.1",
    "archive_sha256": "643e1c7cbb6aec1d920332fe51a7c0d8219e273dcb8602db03f5263e4d16487b",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "fb77efa5445bbb86630c5edbf070d7dbbd475cb0d9201a2e51af9417bacf08f6",
        "index_record_sha256": "bf24ce8d229cb894edd17cd036b1bd0626e2759b34f6749e40c67b121c554293",
        "cksum": "643e1c7cbb6aec1d920332fe51a7c0d8219e273dcb8602db03f5263e4d16487b",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.16.0",
    "archive_sha256": "0e23b1d2c4bd39a41971f6124cef4cc6fd0540913ecb90919b69ab3bbe44ae1a",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "fb77efa5445bbb86630c5edbf070d7dbbd475cb0d9201a2e51af9417bacf08f6",
        "index_record_sha256": "4ec19168cf77d91e52042c6e2957e5d3c89c8e20db02a0e80f400253b70ef31f",
        "cksum": "0e23b1d2c4bd39a41971f6124cef4cc6fd0540913ecb90919b69ab3bbe44ae1a",
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
        "who": "Ben Dean-Kawamura <bdk@mozilla.com>",
        "criteria": "safe-to-deploy",
        "version": "0.16.0",
        "notes": "This crate contains almost no code, it just defines the macros from `askama_derive`.",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "574ee04be5de12d706a6b08b7ea9be71963075f07a87bb5a61379ba4a4fdeba2"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "5e2b20503f9692c887ccb994c87a5cdfa1856e1f876105801287bd8368d0586f"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "b6ac62ff543ad928a670010317fc993ecb2d83c3fcc38f16300687454d9a383f"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "4ebf97e6e5cb921dfd480c6ecd565173443950e922a7a8f113f01a44241b9302"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "7161e233d4bb343058cb5ff4c88e3b52fb4437971c6d94e709817fbf96896057"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "3894dba6692c465ead41df64f08e0d28fffcede18ceea525752aa9a66bb25eaa"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "a048ad2ba4303b722143d3fc12cd91d271813b25ee371ac93c8a2f1dc43123f5"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "37e3ec5591a34df5d3a658488960666a14e13628a1af7b4adaa36f60706b29d5"
    },
    {
      "side": "base",
      "coverage": "entire source; byte-identical",
      "path": "src/lib.rs",
      "sha256": "b7747735c9131691d2e3c2b97bc76f6654a603aeaad36a536bdd0c59aa3214d5"
    },
    {
      "side": "target",
      "coverage": "entire source; byte-identical",
      "path": "src/lib.rs",
      "sha256": "b7747735c9131691d2e3c2b97bc76f6654a603aeaad36a536bdd0c59aa3214d5"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "5e2b20503f9692c887ccb994c87a5cdfa1856e1f876105801287bd8368d0586f",
    ".rustfmt.toml": "a3754a3582a6ab4247c2ace9f9686f8757f94691b01e86f340234c4c18073a1b",
    "Cargo.lock": "4ebf97e6e5cb921dfd480c6ecd565173443950e922a7a8f113f01a44241b9302",
    "Cargo.toml": "3894dba6692c465ead41df64f08e0d28fffcede18ceea525752aa9a66bb25eaa",
    "Cargo.toml.orig": "37e3ec5591a34df5d3a658488960666a14e13628a1af7b4adaa36f60706b29d5",
    "LICENSE-APACHE": "87cb0d734c723c083e51c825930ff42bce28596b52dee15567f6b28f19c195e3",
    "LICENSE-MIT": "df20e0180764bf5bd76f74d47bc9e8c0069a666401629c390003a1d5eba99c92",
    "README.md": "c4d8f405af136169bbc88004f8936d77fb8f137d3b9a3b6d0863e33a857326ef",
    "_typos.toml": "3c73a10e0f3a854d78b80e5a38bde1f052b80640ed2f47cc8d2a353d268165c2",
    "clippy.toml": "d11998a49687dcc509cdd3c37063f74dc29d006ac05adc0b1e9a20d0a2f4904b",
    "deny.toml": "b7637b42164b1bbba883c422187292cf08ad624691145b5124eaf10edf38c164",
    "src/lib.rs": "b7747735c9131691d2e3c2b97bc76f6654a603aeaad36a536bdd0c59aa3214d5",
    "tomlfmt.toml": "a33c21547346656aead5f46f2f07e1965f4cf0c6f2a16deb8ec06019619267aa"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "574ee04be5de12d706a6b08b7ea9be71963075f07a87bb5a61379ba4a4fdeba2",
    ".rustfmt.toml": "a3754a3582a6ab4247c2ace9f9686f8757f94691b01e86f340234c4c18073a1b",
    "Cargo.lock": "b6ac62ff543ad928a670010317fc993ecb2d83c3fcc38f16300687454d9a383f",
    "Cargo.toml": "7161e233d4bb343058cb5ff4c88e3b52fb4437971c6d94e709817fbf96896057",
    "Cargo.toml.orig": "a048ad2ba4303b722143d3fc12cd91d271813b25ee371ac93c8a2f1dc43123f5",
    "LICENSE-APACHE": "87cb0d734c723c083e51c825930ff42bce28596b52dee15567f6b28f19c195e3",
    "LICENSE-MIT": "df20e0180764bf5bd76f74d47bc9e8c0069a666401629c390003a1d5eba99c92",
    "README.md": "c4d8f405af136169bbc88004f8936d77fb8f137d3b9a3b6d0863e33a857326ef",
    "_typos.toml": "3c73a10e0f3a854d78b80e5a38bde1f052b80640ed2f47cc8d2a353d268165c2",
    "clippy.toml": "d11998a49687dcc509cdd3c37063f74dc29d006ac05adc0b1e9a20d0a2f4904b",
    "deny.toml": "b7637b42164b1bbba883c422187292cf08ad624691145b5124eaf10edf38c164",
    "src/lib.rs": "b7747735c9131691d2e3c2b97bc76f6654a603aeaad36a536bdd0c59aa3214d5",
    "tomlfmt.toml": "a33c21547346656aead5f46f2f07e1965f4cf0c6f2a16deb8ec06019619267aa"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/askama_macros-0.16.1/target/Cargo.toml",
      "sha256": "3894dba6692c465ead41df64f08e0d28fffcede18ceea525752aa9a66bb25eaa",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/askama_macros-0.16.1/target/Cargo.toml.orig",
      "sha256": "37e3ec5591a34df5d3a658488960666a14e13628a1af7b4adaa36f60706b29d5",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/askama_macros-0.16.1/target/src/lib.rs",
      "sha256": "b7747735c9131691d2e3c2b97bc76f6654a603aeaad36a536bdd0c59aa3214d5",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/askama_macros-0.16.1/full.diff",
      "sha256": "cba5be92a9362185617b18c6de52fcc4941b875aedcb8099b5b3b2679d9b7566",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/askama_macros-0.16.1/read-files.json",
      "sha256": "1ac4ab5c5888b6303809c0de6ec2091c48f2f5917cf34672a0042bb8f5240b1c",
      "inspection": "independently checked against archive and changed-member coverage"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/askama_macros-0.16.1/target/README.md",
      "sha256": "c4d8f405af136169bbc88004f8936d77fb8f137d3b9a3b6d0863e33a857326ef",
      "inspection": "whole; include_str documentation resource"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "cba5be92a9362185617b18c6de52fcc4941b875aedcb8099b5b3b2679d9b7566",
  "primary_report_sha256": "761c677d09e8e62a57ca3c0ef28ebad025a2e3f1afac4a1c21e56e38a4311d46",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# askama_macros-0.16.1: delta 0.16.0 → 0.16.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: mozilla:0.16.0. `inventory.json` preserves original records, source attribution and both archive checksums; `preflight.json` records source commits, blobs, SHA-256 and frozen lock. Every record explicitly contains safe-to-deploy and the path is contiguous.

- `src/lib.rs:1-18` is byte-identical between versions: the procedural macro crate invokes `askama_derive::make_derive_template!` and `make_filter_fn!`, providing `extern crate askama` to their bodies. `Cargo.toml:58-69` changes the required exact dependency from `=0.16.0` to `=0.16.1`, retaining `external-sources`, `proc-macro`, and disabled dependency defaults; features at 37-56 still forward to that dependency. The actual macro expansion, file access and deployed generated template code belong to `askama_derive`, whose new version MUST be independently audited and is not certified here.
- Package-local code, feature forwarding, edition 2024 and MSRV 1.88 otherwise do not change. There is no build script or binary in either archive. Package Cargo.lock changes include parser and syn major version, but the project lock controls resolution. The frozen App lock selects `askama_derive 0.16.1` and `syn 3.0.3`, not necessarily the bundled lock's syn 3.0.4. This record makes no claim that the new dependency expansion is safe.

All changed hunks of every added/deleted/changed file were read in `full.diff` (SHA-256 `cba5be92a9362185617b18c6de52fcc4941b875aedcb8099b5b3b2679d9b7566`), with six lines of context; `read-files.json` lists source pairs and their hashes. Unchanged files rely on the cited base. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.askama_macros]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.16.0 -> 0.16.1"
```


## Independent cross-review

The 18-line src/lib.rs is byte-identical to the authenticated 0.16.0 base: it forwards proc-macro tokens to askama_derive. Cargo.toml:63-69 pins askama_derive =0.16.1 instead of =0.16.0; features, edition 2024 and MSRV 1.88 are unchanged. The dependency, its file access, and generated code require their own review; this accepts the wrapper delta only.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
