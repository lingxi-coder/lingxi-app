# Source review: zerofrom 0.1.6 -> 0.1.8

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.1.8",
    "archive_sha256": "0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "551c4d8ddb2e429762d0f64b871e05b784207767810e55b626c700213fbadf25",
        "index_record_sha256": "958c474562ac16dd1e589d8407247cb4f455d3e33fe2d165f057a2b2a74d4d05",
        "cksum": "0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.1.6",
    "archive_sha256": "50cc42e0333e05660c3587f3bf9d0478688e15d870fab3346451ce7f8c9fbea5",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "551c4d8ddb2e429762d0f64b871e05b784207767810e55b626c700213fbadf25",
        "index_record_sha256": "620efcd73842ec7bed649e628012daafef1091d225830ef5cea5a9c3f01c53e3",
        "cksum": "50cc42e0333e05660c3587f3bf9d0478688e15d870fab3346451ce7f8c9fbea5",
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
    },
    {
      "source": "google",
      "url": "https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml",
      "source_sha256": "eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd",
      "original_record": {
        "who": "Daniel Cheng <dcheng@chromium.org>",
        "criteria": [
          "safe-to-deploy",
          "does-not-implement-crypto",
          "ub-risk-0"
        ],
        "delta": "0.1.5 -> 0.1.6",
        "notes": "Only minor cfg tweaks.",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "fe0a4ab0dbe6e4ac4efecd96eee237d4e3b47ad3f195834ff6339b6138f92caa",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.lock",
      "sha256": "0ef3e8741dcf5421651406ef74c44b94e8968860dcbe024dd58f6609bfc47e3b",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "83a794ae5d54368a14328428b87d7dfbacedfce5af37cd8f6ef7486db492916b",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "89f802b2f719dc4cb3e512937c4a802490cf43f415f8d4057e5c09dfd04069d3",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "8dea607c122c1f81251643a6dc6614d6065a29781f4425b758228e5c90752b58",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "7d2d0356ffb28fb2513764d6f21dd3b058b65fc3537c18886d47373d818b9128",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "e1b1d7bb66a116d8a299015225104a2acc69c617aab9c285e81fa5d0186ff4e9",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "df8e84cb1898a8daae112228b118fed4861c8c3e9174bb761baf041790c1235a",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "a2ed975e7905fad909f08993e8b2ab10f099c30c045c815c915790cfd77a0cb9",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "b9a524b87596ff7a235e7440f09c34e2368db8b7df48fd7bd3d19bc622413c7f",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "src/zero_from.rs",
      "sha256": "a41c82c9b6552880662b2ed67ada52e8c91611a90699a8f1ccb7035f4efcc85f",
      "scope": "whole source"
    },
    {
      "role": "target",
      "path": "src/macro_impls.rs",
      "sha256": "6b4b69bdc6b94f2abb64da0c2520d3ff74aff7d14e8b4de9e8022d7ddc951f38",
      "scope": "whole source"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "7d2d0356ffb28fb2513764d6f21dd3b058b65fc3537c18886d47373d818b9128",
    "Cargo.lock": "e1b1d7bb66a116d8a299015225104a2acc69c617aab9c285e81fa5d0186ff4e9",
    "Cargo.toml": "df8e84cb1898a8daae112228b118fed4861c8c3e9174bb761baf041790c1235a",
    "Cargo.toml.orig": "a2ed975e7905fad909f08993e8b2ab10f099c30c045c815c915790cfd77a0cb9",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "cd9b9a57dff5012642ee535c46a9f05424eab903f0b9f62de8bf7f815bcd5082",
    "src/lib.rs": "b9a524b87596ff7a235e7440f09c34e2368db8b7df48fd7bd3d19bc622413c7f",
    "src/macro_impls.rs": "6b4b69bdc6b94f2abb64da0c2520d3ff74aff7d14e8b4de9e8022d7ddc951f38",
    "src/zero_from.rs": "a41c82c9b6552880662b2ed67ada52e8c91611a90699a8f1ccb7035f4efcc85f"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "fe0a4ab0dbe6e4ac4efecd96eee237d4e3b47ad3f195834ff6339b6138f92caa",
    "Cargo.lock": "0ef3e8741dcf5421651406ef74c44b94e8968860dcbe024dd58f6609bfc47e3b",
    "Cargo.toml": "83a794ae5d54368a14328428b87d7dfbacedfce5af37cd8f6ef7486db492916b",
    "Cargo.toml.orig": "89f802b2f719dc4cb3e512937c4a802490cf43f415f8d4057e5c09dfd04069d3",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "cd9b9a57dff5012642ee535c46a9f05424eab903f0b9f62de8bf7f815bcd5082",
    "src/lib.rs": "8dea607c122c1f81251643a6dc6614d6065a29781f4425b758228e5c90752b58",
    "src/macro_impls.rs": "6b4b69bdc6b94f2abb64da0c2520d3ff74aff7d14e8b4de9e8022d7ddc951f38",
    "src/zero_from.rs": "a41c82c9b6552880662b2ed67ada52e8c91611a90699a8f1ccb7035f4efcc85f"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/src/lib.rs",
      "sha256": "b9a524b87596ff7a235e7440f09c34e2368db8b7df48fd7bd3d19bc622413c7f"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/src/zero_from.rs",
      "sha256": "a41c82c9b6552880662b2ed67ada52e8c91611a90699a8f1ccb7035f4efcc85f"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/src/macro_impls.rs",
      "sha256": "6b4b69bdc6b94f2abb64da0c2520d3ff74aff7d14e8b4de9e8022d7ddc951f38"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/Cargo.toml",
      "sha256": "df8e84cb1898a8daae112228b118fed4861c8c3e9174bb761baf041790c1235a"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/Cargo.toml.orig",
      "sha256": "a2ed975e7905fad909f08993e8b2ab10f099c30c045c815c915790cfd77a0cb9"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/zerofrom-0.1.8/scratch/target/Cargo.lock",
      "sha256": "e1b1d7bb66a116d8a299015225104a2acc69c617aab9c285e81fa5d0186ff4e9"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "f42a13d80e10c18d98b45750ebdd16c6c3dec6f7bdbd01b65e8666cf8d23a0b0",
  "primary_report_sha256": "93c24b750919f7ebd14d4d70abdd5d3501c279911c416b3c79b89f530141e51a",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# zerofrom 0.1.8: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `0ec05a11813ea801ff6d75110ad09cd0824ddba17dfe17128ea0d5f68e6c5272` matches the product-lock and registry-index checksums; base `0.1.6` archive `50cc42e0333e05660c3587f3bf9d0478688e15d870fab3346451ce7f8c9fbea5` matches the registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of each trusted safe-to-deploy baseline link. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `f42a13d80e10c18d98b45750ebdd16c6c3dec6f7bdbd01b65e8666cf8d23a0b0`; its changed-file set matches the frozen queue. `read-files.json` lists actually reviewed changed files and context with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/lib.rs:5-24` moves crate documentation, changes lint placement and adds a missing-docs warning; module exports and feature gates are otherwise identical. Hashes confirm `zero_from.rs` and `macro_impls.rs` are unchanged; they use lifetime-bound borrowed references, safe copy/conversion implementations and fixed array/tuple macro indices, with no new unsafe code.
- `Cargo.toml:12-71` reduces the declared rust-version from 1.81 to 1.71.1 and raises the optional zerofrom-derive requirement from 0.1.3 to 0.1.6; `alloc` and `derive` feature selection are unchanged. That proc macro can generate code and is separately auditable. The frozen product lock selects zerofrom-derive 0.1.8; the packaged local lock lists 0.1.7 and does not set the product resolution.
- The include list newly names `build.rs`, but no such file is in either verified archive and `build = false`. Remaining manifest changes are lints, package metadata and declaration order; `.cargo_vcs_info.json` changes revision. No packaged generated/native/binary file is added.

## Criteria and boundaries

All authenticated baseline links explicitly carry safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer's judgment the source delta preserves those criteria. A separately audited proc macro is still required when derive is enabled; its behavior is not certified here. The lower declared MSRV was not validated by compiling on that toolchain. No added network, process, file, env, FFI or credential operation occurs in the reviewed delta. Dependencies must satisfy their own audits. This is static review, not formal verification or all-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.zerofrom]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.1.6 -> 0.1.8"
notes = "Full package delta reviewed; source code changes limited to documentation and lint attributes; optional derive minimum rises and requires separate review."
```


## Independent cross-review

Complete 0.1.6 -> 0.1.8 package delta read: src/lib.rs changes only documentation placement/lints; src/zero_from.rs and src/macro_impls.rs are byte-identical across versions. Read the unchanged trait and macro implementations: ZeroFrom values are tied to the input lifetime; Cow, Option, tuple and array conversions borrow or recurse, and integer conversions copy without introducing unsafe or ambient IO. Manifest changes declared MSRV 1.81 -> 1.71.1 and optional zerofrom-derive minimum 0.1.3 -> 0.1.6; package lock chooses 0.1.7 while frozen product lock chooses 0.1.8. Though include mentions build.rs, build=false and the archive contains no build.rs. The optional derive is procedural macro code and must be independently audited at the actual resolved version; this does not certify it. Authenticated google full 0.1.5 followed by 0.1.5 -> 0.1.6 safe-to-deploy delta is a continuous base.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
