# Source review: stable_deref_trait 1.2.0 -> 1.2.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "1.2.1",
    "archive_sha256": "6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "6ea841d5e85df1ecdf39e50137a555d09004c225709acd2270727a558c927c55",
        "index_record_sha256": "1dbfe49c9abc8904e4b3a43e19c3a5c3dafaed5b50f5c1bdea392834261d7647",
        "cksum": "6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "1.2.0",
    "archive_sha256": "a8f112729512f8e442d81f95a8a7ddf2b7c6b8a1a6f509a95864142b30cab2d3",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "6ea841d5e85df1ecdf39e50137a555d09004c225709acd2270727a558c927c55",
        "index_record_sha256": "72561714cb633fb898a4e1f73438344437ac0d4d2fc6ffcfbc7a62ad7a9a3de6",
        "cksum": "a8f112729512f8e442d81f95a8a7ddf2b7c6b8a1a6f509a95864142b30cab2d3",
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
          "ub-risk-2"
        ],
        "version": "1.2.0",
        "notes": "Purely a trait, crates using this should be carefully vetted since self-referential stuff can be super tricky around various unsafe rust edges.",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "9239839d3b7e616ec675a93da9b9a7861c7bf4df7da9365f3273e557585842c6",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "5a8352eba01791ecee28b37cfe1324fa48db52e35023b23a4f07ca84267abfd6",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "b46a09c5bab6e4239b9b2d4dc17516301eda0ad8af4d0c3d39d8758e9d8a5a47",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "c68d43e0ec2727653cc2d37c396c8e60c9ed315b71aab331da8177b129887591",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "e58d6ca3bc691221bb37ea20808b6168e7120494124e9b0aa7080acff3236c57",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "81f573ca2ff7a97a07356e6676fc8ec0cd5ad645d79453329147899c92b82e03",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "5cddd84a243940e60e772ecba2f05b28b00cbd825d341d9e0b0881dcae7847dc",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "e0d471ba7acef517f26f4c34c4017d78436999bcfdbd9f11338df3fce732cc7f",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "4736941fa6c790367158082cb55e5fc89aba7c7f0a85405aada8271fbb5c87ad",
      "scope": "whole changed file; old source reconstructed from target and full delta when appropriate"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "e58d6ca3bc691221bb37ea20808b6168e7120494124e9b0aa7080acff3236c57",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    "Cargo.lock": "81f573ca2ff7a97a07356e6676fc8ec0cd5ad645d79453329147899c92b82e03",
    "Cargo.toml": "5cddd84a243940e60e772ecba2f05b28b00cbd825d341d9e0b0881dcae7847dc",
    "Cargo.toml.orig": "e0d471ba7acef517f26f4c34c4017d78436999bcfdbd9f11338df3fce732cc7f",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7",
    "README.md": "750fc2d57412de57566289e567b5674a666cc7a8a054f8c7129b8553f9003de7",
    "src/lib.rs": "4736941fa6c790367158082cb55e5fc89aba7c7f0a85405aada8271fbb5c87ad"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "9239839d3b7e616ec675a93da9b9a7861c7bf4df7da9365f3273e557585842c6",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    "Cargo.toml": "5a8352eba01791ecee28b37cfe1324fa48db52e35023b23a4f07ca84267abfd6",
    "Cargo.toml.orig": "b46a09c5bab6e4239b9b2d4dc17516301eda0ad8af4d0c3d39d8758e9d8a5a47",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "5e05b024f653a5ce199e77cbbbd42fb5553562ec714b819421ed0c3e552a75d7",
    "README.md": "750fc2d57412de57566289e567b5674a666cc7a8a054f8c7129b8553f9003de7",
    "src/lib.rs": "c68d43e0ec2727653cc2d37c396c8e60c9ed315b71aab331da8177b129887591"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/stable_deref_trait-1.2.1/scratch/target/src/lib.rs",
      "sha256": "4736941fa6c790367158082cb55e5fc89aba7c7f0a85405aada8271fbb5c87ad"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/stable_deref_trait-1.2.1/scratch/target/Cargo.toml",
      "sha256": "5cddd84a243940e60e772ecba2f05b28b00cbd825d341d9e0b0881dcae7847dc"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/stable_deref_trait-1.2.1/scratch/target/Cargo.toml.orig",
      "sha256": "e0d471ba7acef517f26f4c34c4017d78436999bcfdbd9f11338df3fce732cc7f"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/stable_deref_trait-1.2.1/scratch/target/Cargo.lock",
      "sha256": "81f573ca2ff7a97a07356e6676fc8ec0cd5ad645d79453329147899c92b82e03"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "8678818ce7fda1d1a4b604325e2264cb533b65d08ad8d073c77a101a4dd4b241",
  "primary_report_sha256": "cd41d0ed76b9cf0b79a30a89015db8eb2a1e37d7baa8f1217cc717e41d45755a",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# stable_deref_trait 1.2.1: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `6ce2be8dc25455e1f91df71bfa12ad37d7af1092ae736f3a6cd0e37bc7810596` matches the product-lock and registry-index checksums; base `1.2.0` archive `a8f112729512f8e442d81f95a8a7ddf2b7c6b8a1a6f509a95864142b30cab2d3` matches the registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of each trusted safe-to-deploy baseline link. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `8678818ce7fda1d1a4b604325e2264cb533b65d08ad8d073c77a101a4dd4b241`; its changed-file set matches the frozen queue. `read-files.json` lists actually reviewed changed files and context with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/lib.rs:28-35,121-129` describes the unsafe StableDeref contract: moving the owner and shared operations must not invalidate or change its dereference target; CloneStableDeref additionally promises clone address identity. `src/lib.rs:171-180` adds StableDeref for Cow of str, [T], Path, CStr and OsStr, but not CloneStableDeref. A borrowed Cow dereferences the borrowed object, and an owned Cow dereferences its String, Vec, PathBuf, CString or OsString allocation; these owned types already have StableDeref implementations in the base at lines 158-169. Cow mutation through `to_mut(&mut self)` is outside the trait's arbitrary `&self`/`deref_mut` guarantee; the new impls do not assert that cloned owned buffers have the same address. `T: Clone` is needed for Cow<[T]>'s owned form.
- `src/lib.rs:139-140,186-189` gates Arc imports and impls on pointer-width atomics in addition to alloc; this removes implementations where Arc is unavailable, without weakening their safety where present. No executable file outside this Rust source changes.
- `Cargo.toml:12-41` explicitly declares the same library and disables target autodiscovery and build script; default std and alloc features are unchanged, with no dependencies. No rust-version is declared. The package-local lock contains only the package; license spelling, doc URL, and VCS metadata change.

## Criteria and boundaries

All authenticated baseline links explicitly carry safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer's judgment the source delta preserves those criteria. The unsafe trait implementations are assessed against the documented pointer-stability contract, not against every future standard-library implementation or platform. No cross-target build or formal proof was performed; no new external capability is added. Dependencies must satisfy their own audits. This is static review, not formal verification or all-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.stable_deref_trait]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "1.2.0 -> 1.2.1"
notes = "Full delta reviewed, including new Cow StableDeref invariants and target atomic gate; no CloneStableDeref added."
```


## Independent cross-review

Complete 1.2.0 -> 1.2.1 delta read, including manifest, build discovery and lock. At src/lib.rs:28-35 and 121-129 StableDeref requires same deref address throughout moves and shared-method calls; CloneStableDeref additionally requires a clone to refer to the same address. Added unsafe impls at 171-180 cover Cow<str>, Cow<[T]>, Cow<Path>, Cow<CStr>, Cow<OsStr>: borrowed data outlives Cow; owned values keep their allocation when Cow moves. Cow does not implement DerefMut and to_mut requires &mut self, outside this trait's shared-method guarantee. No CloneStableDeref impl is added for Cow, so cloning an owned Cow need not preserve its allocation. Existing Arc implementations are newly gated on pointer atomics and alloc, removing unavailable-target implementations rather than broadening an unsafe impl. No dependencies or new build/native/IO paths. The standard-library type contract is an assumption; no all-platform or formal soundness proof. Authenticated google full 1.2.0 satisfies safe-to-deploy base.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
