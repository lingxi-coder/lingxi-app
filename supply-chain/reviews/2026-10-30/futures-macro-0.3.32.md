# Source review: futures-macro 0.3.31 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "e835b70203e41293343137df5c0664546da5745f82ec9b84d40be8336958447b",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "cea3fe6a17197bbef1284980aa8e6d4a09b4d6cdc60c668ae15187ad6b5b53fb",
        "index_record_sha256": "b037ed4ad99ecee13156463fd85d6614f5c9a820745b3bf98fd070c080a16472",
        "cksum": "e835b70203e41293343137df5c0664546da5745f82ec9b84d40be8336958447b",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.31",
    "archive_sha256": "162ee34ebcb7c64a8abebc059ce0fee27c2262618d7b60ed8faf72fef13c3650",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "cea3fe6a17197bbef1284980aa8e6d4a09b4d6cdc60c668ae15187ad6b5b53fb",
        "index_record_sha256": "5593a830a6ec52e3fd3eb3fb7e3d15ec512ae53ba84801b7b240e808bacd7d65",
        "cksum": "162ee34ebcb7c64a8abebc059ce0fee27c2262618d7b60ed8faf72fef13c3650",
        "yanked": false
      }
    ]
  },
  "baseline_chain": [
    {
      "source": "bytecode-alliance",
      "url": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/04f0fcf70248909cc0b6a3ec1d406c7e2d543637/supply-chain/audits.toml",
      "source_sha256": "71f9f1e8a51d25708f2bb152f39e90a6bdc3d17d91d0f5df4cbc608cef465fd2",
      "original_record": {
        "who": "Joel Dice <joel.dice@gmail.com>",
        "criteria": "safe-to-deploy",
        "version": "0.3.31"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "d756570be752430a116215379f3a4e2e977bdfd01c54fcccbced429b7adf23f7",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "4dfabda19e2ecd80b21a6c87f6f0a7209cc09c3110199ca3e25a9ef46b5b636d",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "d9a6f6b96efb6ab548bb32fff351d41fec2ea5eff447fb1561d241fb844541bb",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "src/select.rs",
      "sha256": "0e641efcdf40f7e0f6ae2b22cb7372c0f68472aa6942d7099091ce4d43004973",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "8e043c81af3d1cef6559df7d7a8cdd30a9c7a004ac72c783e9e7b428c9503b28",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "cbe930850a4af4ac1786e649cb4c2233aaba4d4ed429616442bf71caab97589e",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "2dc3d7322eb40cae2c4c15252e814675ef6ff1a33dbd5706d2941351d195d02a",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "a9616e10d5ea9f36f5aaba41d42c758e78204d672354273f0ea7efff543e4593",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "src/select.rs",
      "sha256": "2d4767d640ad227fd5d5ad437da978bec32e38b24639a0a0e7064abb82ff9a1b",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "8e043c81af3d1cef6559df7d7a8cdd30a9c7a004ac72c783e9e7b428c9503b28",
    "Cargo.lock": "cbe930850a4af4ac1786e649cb4c2233aaba4d4ed429616442bf71caab97589e",
    "Cargo.toml": "2dc3d7322eb40cae2c4c15252e814675ef6ff1a33dbd5706d2941351d195d02a",
    "Cargo.toml.orig": "a9616e10d5ea9f36f5aaba41d42c758e78204d672354273f0ea7efff543e4593",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "src/executor.rs": "d2c8545c3438262484da3cddb6d998928bfef8d6d191480c13c0c390f15fe0f1",
    "src/join.rs": "e60b42045401b16bf27b4b1ad5bbbea16348cd9db591b309e56a8987850c479a",
    "src/lib.rs": "dc139b7703637521a39846beb8ecbfbaf836baa5b8bbc3a25aad0880b3103d29",
    "src/select.rs": "2d4767d640ad227fd5d5ad437da978bec32e38b24639a0a0e7064abb82ff9a1b",
    "src/stream_select.rs": "5fb84834a40876ab1fd975c3af67594d0c5a4f8d724cb164db9bee71e70d14b1"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "d756570be752430a116215379f3a4e2e977bdfd01c54fcccbced429b7adf23f7",
    "Cargo.toml": "4dfabda19e2ecd80b21a6c87f6f0a7209cc09c3110199ca3e25a9ef46b5b636d",
    "Cargo.toml.orig": "d9a6f6b96efb6ab548bb32fff351d41fec2ea5eff447fb1561d241fb844541bb",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "src/executor.rs": "d2c8545c3438262484da3cddb6d998928bfef8d6d191480c13c0c390f15fe0f1",
    "src/join.rs": "e60b42045401b16bf27b4b1ad5bbbea16348cd9db591b309e56a8987850c479a",
    "src/lib.rs": "dc139b7703637521a39846beb8ecbfbaf836baa5b8bbc3a25aad0880b3103d29",
    "src/select.rs": "0e641efcdf40f7e0f6ae2b22cb7372c0f68472aa6942d7099091ce4d43004973",
    "src/stream_select.rs": "5fb84834a40876ab1fd975c3af67594d0c5a4f8d724cb164db9bee71e70d14b1"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-macro-0.3.32/scratch/target/src/select.rs",
      "sha256": "2d4767d640ad227fd5d5ad437da978bec32e38b24639a0a0e7064abb82ff9a1b"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-macro-0.3.32/scratch/target/Cargo.toml",
      "sha256": "2dc3d7322eb40cae2c4c15252e814675ef6ff1a33dbd5706d2941351d195d02a"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-macro-0.3.32/scratch/target/Cargo.toml.orig",
      "sha256": "a9616e10d5ea9f36f5aaba41d42c758e78204d672354273f0ea7efff543e4593"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-macro-0.3.32/scratch/target/Cargo.lock",
      "sha256": "cbe930850a4af4ac1786e649cb4c2233aaba4d4ed429616442bf71caab97589e"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "ea29930b1ca3d081c3e3c1b14430872b32d5455d0f7258b83cb02e0f07387f9c",
  "primary_report_sha256": "798d933b69f4b5197cbf43633c86e0dba48ea1c249758aa7a3a406c24b3e1b9c",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-macro 0.3.32: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. No audit record has been registered. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review).

## Identity and evidence

- Frozen batch: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; lane queue: `1f7f8c3bc014da44f795bbb3984f7ae191c9a1269cfdeafffbd1dc9a1123ea72`; product Cargo.lock: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
- Target archive `e835b70203e41293343137df5c0664546da5745f82ec9b84d40be8336958447b` matches the frozen Cargo.lock and its crate-index checksum; base `0.3.31` archive `162ee34ebcb7c64a8abebc059ce0fee27c2262618d7b60ed8faf72fef13c3650` matches its crate-index checksum. Both normalized manifests agree with their name/version.
- `verification.json` records original archive and index paths, index-file hashes, every member SHA-256 and mode, and the original records of the trusted full/delta chain ending at `0.3.31`. The source snapshots match their fixed SHA-256 and Git blob IDs. Every path is within the expected package prefix, unique and regular, with no links or binary members.
- `full.diff` is the complete byte-driven textual package diff (ea29930b1ca3d081c3e3c1b14430872b32d5455d0f7258b83cb02e0f07387f9c); its changed-file set equals the frozen queue. `read-files.json` records the reviewed changed files and contextual source with their hashes. Unchanged files are identified by hashing; this is a delta review, not an assertion that the full base was independently re-audited.

## Analysis

- `src/select.rs:221-230,297-322`: the sole generated-code change inserts a literal space between two words in a panic string for the existing all-futures-terminated/no-complete-handler branch. The branch selection, panic, polling and handlers are unchanged. `select.rs:146-216` shows the pre-existing generated `Pin::new_unchecked`: named paths are checked `Unpin`; expression values are held in the generated scope, with the same pinning argument as the authenticated base. This delta does not alter those tokens.
- `Cargo.toml:12-46` raises declared MSRV 1.56 to 1.71, declares `autolib = false` while retaining the explicit procedural-macro library, and moves an empty features table. `proc-macro2`, `quote`, and `syn` requirements/features are unchanged. `build = false` remains. Package-local `Cargo.lock` is added with its own resolved versions; the frozen product lock instead selects proc-macro2 1.0.106, quote 1.0.45 and syn 2.0.117 for the 2.x requirement. Those dependencies require separate audits.
- `.cargo_vcs_info.json` changes revision. All remaining macro source files are byte-identical. The proc macro executes during compilation and can emit deployed code, but the examined delta does not add host permissions, generated unsafe operations, external files, process or network access.

## Criteria and boundaries

The checked baseline path has explicit `safe-to-deploy` on every full/delta link, which also implies `safe-to-run`; see [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) and [delta semantics](https://mozilla.github.io/cargo-vet/audit-entries.html). The above source and manifest delta preserves those criteria in this reviewer’s judgment. MSRV 1.71 is a compatibility constraint, not a verified build on every toolchain. Existing generated unsafe pinning and proc-macro dependencies remain inherited boundaries; they are not independently re-certified here.

Dependent crates need their own assessments; this review does not recursively certify them. It is static source review, not formal verification or cross-platform runtime acceptance. No product, lock, registry, or formal audit store was modified.

## Proposed record, for root review only

```toml
[[audits.futures-macro]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.31 -> 0.3.32"
notes = "Full delta and select macro expansion context reviewed; only generated panic text changes; MSRV rises to 1.71, dependencies reviewed separately."
```


## Independent cross-review

Complete 0.3.31 -> 0.3.32 delta includes both manifests, new standalone lock, VCS metadata, and src/select.rs:221-229: the only source edit adds a separating space in the panic message for exhausted futures with no complete handler. The selected result/polling algorithm does not change. Existing generated select code (src/select.rs:147-216) checks a path future is FusedFuture and Unpin before creating Pin::new_unchecked; other future expressions are bound and shadowed locally for the selection lifetime, then polled only when not terminated. This pre-existing unsafe invariant was read, but not exhaustively proven for all macro expansions. Proc-macro dependency version ranges unchanged; MSRV changes 1.56 -> 1.71. Target package has no build script or bundled binary; dependency code is separately audited. Authenticated bytecode-alliance full 0.3.31 provides safe-to-deploy baseline.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
