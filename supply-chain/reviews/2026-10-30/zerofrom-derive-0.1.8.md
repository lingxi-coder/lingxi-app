# Source review: zerofrom-derive 0.1.5 -> 0.1.8

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.1.8",
    "archive_sha256": "f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "98ec12cfa05c0ffbf164281bf67e0acdb701a10175800535ba4e8300fda3a643",
        "index_record_sha256": "69459607c3df05010e59004bd4d75de3ed830d53812de22045d988c1c3a2f710",
        "cksum": "f75b4683f6c7f45248d4d64056a24298c6281e0993356d7d1b4a1a962ef10d4a",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.1.5",
    "archive_sha256": "595eed982f7d355beb85837f651fa22e90b3c044842dc7f2c2842c086f295808",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "064348479841924f720f9f14be95656f3bcfab154120c5da4709029b2f2dae4e",
        "index_record_sha256": "b2c4a8c29d39baf99fec82f7dca4a5b7aac271c11bad4998be4a674771131c08",
        "cksum": "595eed982f7d355beb85837f651fa22e90b3c044842dc7f2c2842c086f295808",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "98ec12cfa05c0ffbf164281bf67e0acdb701a10175800535ba4e8300fda3a643",
        "index_record_sha256": "b2c4a8c29d39baf99fec82f7dca4a5b7aac271c11bad4998be4a674771131c08",
        "cksum": "595eed982f7d355beb85837f651fa22e90b3c044842dc7f2c2842c086f295808",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "google",
      "url": "https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml",
      "source_sha256": "eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd",
      "original_record": {
        "who": "Manish Goregaokar <manishearth@google.com>",
        "criteria": [
          "safe-to-deploy",
          "does-not-implement-crypto",
          "ub-risk-0"
        ],
        "version": "0.1.5",
        "notes": "Contains no unsafe",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "7c3c1d032d1e6d1d7d9a47bfd40f089c3b86a9c7788d4a5e7d9f45ad9e5dd8ce"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "bc87aba3732b80cc96aff73b42cc1c340361f90188a487ed4b27039e6eea682b"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "fc6219b76e4c1734055495c69f5e3bca1c38f65610a2c70456f1bdd401fb19e8"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "a2768848c33e4e6e7bc66165d70e60397570102afe256d0a29c109b0987fd6f3"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "5af9fbb144d61447f00349f3b4cd4688cbd4317ba87051ddc4f1b384e444e9a8"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "0bc9ebe5d49afbfb7e9a32272ea24dfef28889c2c4cb4f153168f1f1c2bee837"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "0de89a21f269ee1370ea134f4a6d350cb77eff6aa9e4f5bc1fca4d119a5b8102"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "0d5ffb5107c53f300a68a1fea020c8a0d0d46113b3f5c86c099ae530f46950a2"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "examples/zf_derive.rs",
      "sha256": "39342f4658da4c6e475a290028b7cbab161e1fb91ee49c79cda4799a8614bc9b"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "examples/zf_derive.rs",
      "sha256": "9bef98323501d3b23fd3fd17de1a394f6e3e3e8acf08fa0edf53f8945c3e8854"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "2f746c88a50e96e358529ba8f5d73a5941296dacecee405e2b4b1489c6a59ae6"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "a8cd6a9c4f7634539660001146ee078887679655844509c0f46ea5b88c5f69f9"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/visitor.rs",
      "sha256": "6eb54d2e1c4f9211bcaa5d6e768d668063a94411bfbed610b0a0419ac37bf545"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/visitor.rs",
      "sha256": "3317ce40feb6de39a80895739d3eb2e87659f943dc8d42b2eeddae145b0492a8"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "bc87aba3732b80cc96aff73b42cc1c340361f90188a487ed4b27039e6eea682b",
    "Cargo.lock": "a2768848c33e4e6e7bc66165d70e60397570102afe256d0a29c109b0987fd6f3",
    "Cargo.toml": "0bc9ebe5d49afbfb7e9a32272ea24dfef28889c2c4cb4f153168f1f1c2bee837",
    "Cargo.toml.orig": "0d5ffb5107c53f300a68a1fea020c8a0d0d46113b3f5c86c099ae530f46950a2",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "2ef78913a98fb26c912c47c3c96167b9b57524754dc4857fe18a2122fec4985e",
    "examples/zf_derive.rs": "9bef98323501d3b23fd3fd17de1a394f6e3e3e8acf08fa0edf53f8945c3e8854",
    "src/lib.rs": "a8cd6a9c4f7634539660001146ee078887679655844509c0f46ea5b88c5f69f9",
    "src/visitor.rs": "3317ce40feb6de39a80895739d3eb2e87659f943dc8d42b2eeddae145b0492a8"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "7c3c1d032d1e6d1d7d9a47bfd40f089c3b86a9c7788d4a5e7d9f45ad9e5dd8ce",
    "Cargo.lock": "fc6219b76e4c1734055495c69f5e3bca1c38f65610a2c70456f1bdd401fb19e8",
    "Cargo.toml": "5af9fbb144d61447f00349f3b4cd4688cbd4317ba87051ddc4f1b384e444e9a8",
    "Cargo.toml.orig": "0de89a21f269ee1370ea134f4a6d350cb77eff6aa9e4f5bc1fca4d119a5b8102",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "2ef78913a98fb26c912c47c3c96167b9b57524754dc4857fe18a2122fec4985e",
    "examples/zf_derive.rs": "39342f4658da4c6e475a290028b7cbab161e1fb91ee49c79cda4799a8614bc9b",
    "src/lib.rs": "2f746c88a50e96e358529ba8f5d73a5941296dacecee405e2b4b1489c6a59ae6",
    "src/visitor.rs": "6eb54d2e1c4f9211bcaa5d6e768d668063a94411bfbed610b0a0419ac37bf545"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/zerofrom-derive-0.1.8/target/Cargo.toml",
      "sha256": "0bc9ebe5d49afbfb7e9a32272ea24dfef28889c2c4cb4f153168f1f1c2bee837",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/zerofrom-derive-0.1.8/target/Cargo.toml.orig",
      "sha256": "0d5ffb5107c53f300a68a1fea020c8a0d0d46113b3f5c86c099ae530f46950a2",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/zerofrom-derive-0.1.8/target/src/lib.rs",
      "sha256": "a8cd6a9c4f7634539660001146ee078887679655844509c0f46ea5b88c5f69f9",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/zerofrom-derive-0.1.8/target/src/visitor.rs",
      "sha256": "3317ce40feb6de39a80895739d3eb2e87659f943dc8d42b2eeddae145b0492a8",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/zerofrom-derive-0.1.8/full.diff",
      "sha256": "1a1cb2ca71d2786d3919b4c013e0090058fdfce48c3cfc114dfc9e293e54a3ef",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/zerofrom-derive-0.1.8/read-files.json",
      "sha256": "5326a396c68a70312a6978b7794610727ba93f52cfae8a46eaf7662bc1d773d5",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": {
    "purpose": "AST API contract inspection only; not a syn audit",
    "archive_path": "crates.io/syn-3.0.3.crate",
    "archive_sha256": "53e9bae58849f64dfa4f5d5ae372c8341f7305f82a3868709269343628b659a3",
    "project_lock_match": true,
    "files": [
      {
        "source_path": "crates.io/syn-3.0.3/src/generics.rs",
        "sha256": "a563693b6c9ec11d61b2ce7f869763a3c967879dd9306743f01203cd9555cda1",
        "coverage": "relevant AST type fields and pop/push methods"
      },
      {
        "source_path": "crates.io/syn-3.0.3/src/punctuated.rs",
        "sha256": "6b70fd15cc662260c2c0f20cd35a777351b2c5199b299cf5c8d8972723bd0f04",
        "coverage": "relevant AST type fields and pop/push methods"
      }
    ]
  },
  "full_diff_sha256": "1a1cb2ca71d2786d3919b4c013e0090058fdfce48c3cfc114dfc9e293e54a3ef",
  "primary_report_sha256": "3d634841fd490c9c5b0828039f285c22b95897720ae2dbbc1f289b23bc5c837f",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# zerofrom-derive-0.1.8: delta 0.1.5 → 0.1.8

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: google:0.1.5. `inventory.json` preserves original records, source attribution and both archive checksums; `preflight.json` records source commits, blobs, SHA-256 and frozen lock. Every record explicitly contains safe-to-deploy and the path is contiguous.

- `src/lib.rs:87-149`: strips type defaults and filters `?Sized` on `may_borrow` parameters; new syn 3 AST puts `=` and default together in `TypeParam.default`, and exposes the question mark as `TraitBound.maybe`. Removing `default` now removes both, and the loop still removes only `?Sized` while reconstructing the other bounds. Compared the contract to the frozen lock's syn 3.0.3 `src/generics.rs:73-79,421-431` and `src/punctuated.rs:207-215,255-263`; those two source files were byte-compared with the SHA-verified syn archive (their hashes in `dependency-contract.json`). This is contract inspection, not a syn audit.
- `src/lib.rs:151-259`: generated implementations still use safe Clone/Copy or call `ZeroFrom` on fields with compiler-enforced bounds and lifetimes; no unsafe output is added. `src/visitor.rs` only changes documentation. Change to `core::mem::replace` is qualification only. Macro behavior depends on syn/synstructure and `zerofrom`, each separately audited.
- `Cargo.toml` upgrades syn 2 to 3 and synstructure 0.13 to 0.14, enables syn visit, MSRV 1.71.1, no build script; example changes only public visibility. No new C, asm, binary, network, command, or permission logic; package Cargo.lock is not the App lock.

All changed hunks of every added/deleted/changed file were read in `full.diff` (SHA-256 `1a1cb2ca71d2786d3919b4c013e0090058fdfce48c3cfc114dfc9e293e54a3ef`), with six lines of context; `read-files.json` lists source pairs and their hashes. Unchanged files rely on the cited base. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.zerofrom-derive]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.1.5 -> 0.1.8"
```


## Independent cross-review

src/lib.rs:87-149 adapts changed syn AST: clearing default removes the equals token and type; on may_borrow bounds it removes only ?Sized and rebuilds the rest. Independently checked locked syn 3.0.3 generics.rs:73-79,421-431 and punctuated.rs:207-215,255-265 against its archive and frozen lock. Generated safe field-wise ZeroFrom/Clone/Copy paths at :151-259 are unchanged in kind; syn, synstructure and zerofrom remain independent audit obligations. Google full 0.1.5 is the base.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
