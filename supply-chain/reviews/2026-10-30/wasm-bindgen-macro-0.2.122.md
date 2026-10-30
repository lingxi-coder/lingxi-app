# Source review: wasm-bindgen-macro 0.2.122

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.2.122",
    "archive_sha256": "916151b09da36bd82f6615cbf3a419e2f0ba23a03c6160e8e92eb6bd4aa1dec6",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "7553d4618e5fc0aed35c54dd795e4688fee65bbe0284eacc67cc8ad49e0ec628",
        "index_record_sha256": "a5f1480fc28815e1e2e8a2042413ea5338c384c26a54a9b471c7bf2ba7aec335",
        "cksum": "916151b09da36bd82f6615cbf3a419e2f0ba23a03c6160e8e92eb6bd4aa1dec6",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "8c1adb02c9b4e9e2bb3d3b5512605848ce4990ebd59f65918800a6228b649bd0",
        "index_record_sha256": "a5f1480fc28815e1e2e8a2042413ea5338c384c26a54a9b471c7bf2ba7aec335",
        "cksum": "916151b09da36bd82f6615cbf3a419e2f0ba23a03c6160e8e92eb6bd4aa1dec6",
        "yanked": false
      }
    ]
  },
  "base": null,
  "baseline_chain": [],
  "read_files": [
    {
      "path": ".cargo_vcs_info.json",
      "sha256": "95f763d97a262c92ff9c1cc2ef56078b361bc6f3cd913429b7679a753083595d",
      "bytes": 106,
      "kind": "text"
    },
    {
      "path": "Cargo.lock",
      "sha256": "6e0e24877fdd0015aca24aae017fbce2c904fb13b6a5270aa2d3a5709b662370",
      "bytes": 7062,
      "kind": "text"
    },
    {
      "path": "Cargo.toml",
      "sha256": "be2404963b1c345b0c91a612d91cd44f3db9bde0c2c69f5e247e5ef58abb590d",
      "bytes": 1822,
      "kind": "text"
    },
    {
      "path": "Cargo.toml.orig",
      "sha256": "9e12e6bb0e012739ba896458be722f12bdb78ca6b6a5444a92fe05c357cfc20e",
      "bytes": 924,
      "kind": "text"
    },
    {
      "path": "README.md",
      "sha256": "5e8d81c7bfeb6d08e0781bf572b652f064af0bf82eb030ece7af3140b135b0ef",
      "bytes": 1298,
      "kind": "text"
    },
    {
      "path": "src/lib.rs",
      "sha256": "1e0bcfc96e940ed67ec83895f987eb7cfb1e603435f3f9537ec0485154a9c8b5",
      "bytes": 2712,
      "kind": "text"
    },
    {
      "path": "src/worker.js",
      "sha256": "cb5c33dd41cfad622abce5464a3ceae478904ea16ddc3c6c769ee33e0c086692",
      "bytes": 60,
      "kind": "text"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "95f763d97a262c92ff9c1cc2ef56078b361bc6f3cd913429b7679a753083595d",
    "Cargo.lock": "6e0e24877fdd0015aca24aae017fbce2c904fb13b6a5270aa2d3a5709b662370",
    "Cargo.toml": "be2404963b1c345b0c91a612d91cd44f3db9bde0c2c69f5e247e5ef58abb590d",
    "Cargo.toml.orig": "9e12e6bb0e012739ba896458be722f12bdb78ca6b6a5444a92fe05c357cfc20e",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397",
    "README.md": "5e8d81c7bfeb6d08e0781bf572b652f064af0bf82eb030ece7af3140b135b0ef",
    "src/lib.rs": "1e0bcfc96e940ed67ec83895f987eb7cfb1e603435f3f9537ec0485154a9c8b5",
    "src/worker.js": "cb5c33dd41cfad622abce5464a3ceae478904ea16ddc3c6c769ee33e0c086692"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/wasm-bindgen-macro-0.2.122/target/Cargo.toml",
      "sha256": "be2404963b1c345b0c91a612d91cd44f3db9bde0c2c69f5e247e5ef58abb590d",
      "inspection": "whole"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/wasm-bindgen-macro-0.2.122/target/Cargo.toml.orig",
      "sha256": "9e12e6bb0e012739ba896458be722f12bdb78ca6b6a5444a92fe05c357cfc20e",
      "inspection": "whole"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/wasm-bindgen-macro-0.2.122/target/src/lib.rs",
      "sha256": "1e0bcfc96e940ed67ec83895f987eb7cfb1e603435f3f9537ec0485154a9c8b5",
      "inspection": "whole"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/wasm-bindgen-macro-0.2.122/target/src/worker.js",
      "sha256": "cb5c33dd41cfad622abce5464a3ceae478904ea16ddc3c6c769ee33e0c086692",
      "inspection": "whole"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/wasm-bindgen-macro-0.2.122/read-files.json",
      "sha256": "d6be332ca6d11f0963dbfb694f0f7bfe1c9a5a69fac5822b6a1dadabd12ba766",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": null,
  "primary_report_sha256": "c8ce4347b695d60afe69191a22e8d8a842a68b604f1fa0f058548cfb5cd55e56",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# wasm-bindgen-macro-0.2.122

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Proposed full package-local `safe-to-deploy`, subject to independent review; dependencies remain separately gated.

- `src/lib.rs:8-20,35-49,51-75`: four proc-macro entry points forward the input tokens to exact-version `wasm-bindgen-macro-support`; on success returned tokens pass through; on error diagnostics become tokens. The link_to error wraps its diagnostic in `String::clone` to preserve an IDE-visible type. No local unsafe/FFI or command/file/network implementation appears. `cfg!(xxx_debug_only_print_generated_code)` can print tokens during compilation only if explicitly enabled; generated code may include sensitive input, so do not enable it in sensitive builds without considering log exposure.
- `src/worker.js:1` is only a comment for a doctest, not a JS worker implementation. `Cargo.toml:12-27,37-50` declares edition 2021, MSRV 1.77, no build script, one proc macro, quote and exact macro-support 0.2.122, plus an optional strict-macro feature forwarded to support. README and package lock describe documentation/dev dependencies; the frozen project lock determines actual resolution.
- The local dispatcher is statically reviewable for `safe-to-deploy`, but the security, filesystem behavior and safety of generated Rust/JS reside in macro-support and its dependencies; they were NOT recursively audited here and require their own gate. No claim that macro expansions or platform behavior were tested.

Evidence: `inventory.json` records target archive, registry-index and App lock checksums, safe archive member paths, and hashes of all members. `read-files.json` lists source and metadata actually read with SHA-256; license text identified by filename/hash only, not legally reviewed. There was no available existing base for this package in the queue. No platform execution, formal proof or recursive dependency review is claimed.

Proposed record for independent reviewer (not registered):

```toml
[[audits.wasm-bindgen-macro]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
version = "0.2.122"
```


## Independent cross-review

Whole src/lib.rs:8-20,35-75 passes tokens to macro-support, surfaces diagnostic tokens, and optional debug cfg prints generated code. Cargo.toml:12-27,37-50 contains exact macro-support and quote, build=false and proc-macro=true. src/worker.js is only a doctest comment. Caller inputs/debug logging and independently gated macro-support/generated output remain explicit boundaries.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
