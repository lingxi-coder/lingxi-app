# Source review: num-conv 0.2.1 -> 0.2.2

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.2.2",
    "archive_sha256": "521739c6d2bac4aa25192232afe6841231376b2b26d4d9fae5ecf8ca5772e441",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f4596dc014d4960a099e830475f07fbc9dbb8c6994dfa44f1aef796a8231b4e7",
        "index_record_sha256": "068628f5e3f250931b5b212c8067e32ad2eb955fbda7734b5af92da00970380b",
        "cksum": "521739c6d2bac4aa25192232afe6841231376b2b26d4d9fae5ecf8ca5772e441",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.2.1",
    "archive_sha256": "c6673768db2d862beb9b39a78fdcb1a69439615d5794a1be50caa9bc92c81967",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "f4596dc014d4960a099e830475f07fbc9dbb8c6994dfa44f1aef796a8231b4e7",
        "index_record_sha256": "89889bc28cfce7c7f87905e6a4019e766e3405509c618a711278cd7137982d8f",
        "cksum": "c6673768db2d862beb9b39a78fdcb1a69439615d5794a1be50caa9bc92c81967",
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
        "who": "Alex Franchuk <afranchuk@mozilla.com>",
        "criteria": "safe-to-deploy",
        "version": "0.1.0",
        "notes": "Very straightforward, simple crate. No dependencies, unsafe, extern,\nside-effectful std functions, etc.\n",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Lars Eggert <lars@eggert.org>",
        "criteria": "safe-to-deploy",
        "delta": "0.1.0 -> 0.2.0",
        "notes": "Revision only removes code",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "bytecode-alliance",
      "url": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/04f0fcf70248909cc0b6a3ec1d406c7e2d543637/supply-chain/audits.toml",
      "source_sha256": "71f9f1e8a51d25708f2bb152f39e90a6bdc3d17d91d0f5df4cbc608cef465fd2",
      "original_record": {
        "who": "Alex Crichton <alex@alexcrichton.com>",
        "criteria": "safe-to-deploy",
        "delta": "0.2.0 -> 0.2.1",
        "notes": "Minor update, nothing major"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "6becb85ebecec0de0ad1580593000f3c920939eafd52707799403645a36a059a",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.lock",
      "sha256": "0761cb38fffefd4118ba3d7c12bbabe37c1552fde772adb88d35f3e31050b206",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "c6d982b10de304bce8e85b8682a7074f8d697c2410bca7644fecc018f4c39b34",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "bf02eb348d29d448ad35e35c17145418c4267ce01238c68526b1e0ff980919c8",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "README.md",
      "sha256": "081b57a3244bd2d42ae1c9592c90556bf50fc2155a53760d9049c9dacbe92354",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "9c7e451a50cc22348eba76ebc40f27a67d4bfe6a271af17c5379f02a95b8f323",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "d6796518ecb4a22c4fa88353e4fa621353f944a0a79a61152c419bdc0b8a1ab1",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "5e8bc520f9f81d5820f9e46d46f1abf22a6d402ba29a74292eb422aa3f5f7194",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "ecd9eaa1fb85eb5c739bce95bf50387e018e3b3f0d109e97416111acbb190028",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "59aa824bbf80cc75cd8c3056f67c1ca43b9452130831d53d907c967790128531",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "README.md",
      "sha256": "b06b19b8a6dec1dfa913e035ba9f99e04fd6a1a170a86fa03d9d3b9fb03c7de0",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "3d341469c5f036f5b7aca71752a24cfd3d834ecfab1c570b26eb6a2676a8e388",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "d6796518ecb4a22c4fa88353e4fa621353f944a0a79a61152c419bdc0b8a1ab1",
    "Cargo.lock": "5e8bc520f9f81d5820f9e46d46f1abf22a6d402ba29a74292eb422aa3f5f7194",
    "Cargo.toml": "ecd9eaa1fb85eb5c739bce95bf50387e018e3b3f0d109e97416111acbb190028",
    "Cargo.toml.orig": "59aa824bbf80cc75cd8c3056f67c1ca43b9452130831d53d907c967790128531",
    "LICENSE-Apache": "0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594",
    "LICENSE-MIT": "e2e245f2b566d0bfafb0f6a04c16b82d710c4e6e7d799e39bb4ef6e541b85b98",
    "README.md": "b06b19b8a6dec1dfa913e035ba9f99e04fd6a1a170a86fa03d9d3b9fb03c7de0",
    "src/lib.rs": "3d341469c5f036f5b7aca71752a24cfd3d834ecfab1c570b26eb6a2676a8e388"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "6becb85ebecec0de0ad1580593000f3c920939eafd52707799403645a36a059a",
    "Cargo.lock": "0761cb38fffefd4118ba3d7c12bbabe37c1552fde772adb88d35f3e31050b206",
    "Cargo.toml": "c6d982b10de304bce8e85b8682a7074f8d697c2410bca7644fecc018f4c39b34",
    "Cargo.toml.orig": "bf02eb348d29d448ad35e35c17145418c4267ce01238c68526b1e0ff980919c8",
    "LICENSE-Apache": "0d542e0c8804e39aa7f37eb00da5a762149dc682d7829451287e11b938e94594",
    "LICENSE-MIT": "e2e245f2b566d0bfafb0f6a04c16b82d710c4e6e7d799e39bb4ef6e541b85b98",
    "README.md": "081b57a3244bd2d42ae1c9592c90556bf50fc2155a53760d9049c9dacbe92354",
    "src/lib.rs": "9c7e451a50cc22348eba76ebc40f27a67d4bfe6a271af17c5379f02a95b8f323"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/num-conv-0.2.2/scratch/target/src/lib.rs",
      "sha256": "3d341469c5f036f5b7aca71752a24cfd3d834ecfab1c570b26eb6a2676a8e388"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/num-conv-0.2.2/scratch/target/Cargo.toml",
      "sha256": "ecd9eaa1fb85eb5c739bce95bf50387e018e3b3f0d109e97416111acbb190028"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/num-conv-0.2.2/scratch/target/Cargo.toml.orig",
      "sha256": "59aa824bbf80cc75cd8c3056f67c1ca43b9452130831d53d907c967790128531"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/num-conv-0.2.2/scratch/target/Cargo.lock",
      "sha256": "5e8bc520f9f81d5820f9e46d46f1abf22a6d402ba29a74292eb422aa3f5f7194"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "7a13f2249b70da8bb1d12813c839ab3ee877b8bd553e9b666b6079f03c5d8a5e",
  "primary_report_sha256": "27fb96a9e45dfe713c321f51021c0504ed6ae547825c1eef0158b8f36e9d7d8c",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# num-conv 0.2.2: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `521739c6d2bac4aa25192232afe6841231376b2b26d4d9fae5ecf8ca5772e441` matches the product-lock and registry-index checksums; base `0.2.1` archive `c6673768db2d862beb9b39a78fdcb1a69439615d5794a1be50caa9bc92c81967` matches the registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of each trusted safe-to-deploy baseline link. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `7a13f2249b70da8bb1d12813c839ab3ee877b8bd553e9b666b6079f03c5d8a5e`; its changed-file set matches the frozen queue. `read-files.json` lists actually reviewed changed files and context with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/lib.rs:17-19,36-44,53-65,73-103` exports a new Widen trait alongside a deprecated Extend. `src/lib.rs:240-274` generates Widen and legacy Extend through the same input/output pairs, using the same primitive `as` conversion; a compile-time size check rejects wider-to-narrower pairs. The matrix at lines 324-338 preserves signed-to-signed and unsigned-to-unsigned pairs, including architecture-sized integers. This is value-preserving for the permitted widening pairs; no unsafe code or arbitrary user-provided conversion implementations are added because the input trait is sealed.
- The existing truncate implementation is unchanged; deprecation of Extend can surface warnings for consumers that deny warnings. `Cargo.toml:12-56` retains MSRV 1.57.0, no dependencies, no build script, same no-std library and feature set. The local lock and manifest version change, VCS identity changes, and README/doc additions link a tracking issue; there is no new code-generating resource.

## Criteria and boundaries

All authenticated baseline links explicitly carry safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer's judgment the source delta preserves those criteria. Static comparison does not prove all target architectures or compiler MSRV builds. Deprecation can affect warning-denying consumers; it does not add ambient capabilities. Dependencies must satisfy their own audits. This is static review, not formal verification or all-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.num-conv]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.2.1 -> 0.2.2"
notes = "Full package delta reviewed; new sealed Widen mirrors existing Extend conversion pairs and checks; legacy Extend is deprecated."
```


## Independent cross-review

Read all 0.2.1 -> 0.2.2 files in the complete delta and src/lib.rs. New sealed WidenTarget/Widen APIs and impl_widen! at 237-270 generate the same as-cast for the old Extend method and new Widen method. Explicit generated pairs at 324-338 keep signedness and select destination sizes >= source; const size assertions reject unsupported target-width combinations, preventing silent narrowing. Existing Truncate code is unchanged. Extend remains but is deprecated, so users treating deprecations as errors may need a source update. Manifest keeps MSRV 1.57, no dependencies, build=false and self-only lock; no unsafe, FFI or ambient IO added. Authenticated mozilla full 0.1.0 -> 0.2.0 and bytecode-alliance 0.2.0 -> 0.2.1 provide the safe-to-deploy base. Static review is not exhaustive execution across unusual target widths.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
