# Source review: futures-executor 0.3.31 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "baf29c38818342a3b26b5b923639e7b1f4a61fc5e76102d4b1981c6dc7a7579d",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "489639febb8aba64115e19ffa11fed0601c30bab99022b8711dc1909ae80d34c",
        "index_record_sha256": "31367525de3b159372c005f615ccba60e12dd3be97e74988b58b957dea9d22a7",
        "cksum": "baf29c38818342a3b26b5b923639e7b1f4a61fc5e76102d4b1981c6dc7a7579d",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.31",
    "archive_sha256": "1e28d1d997f585e54aebc3f97d39e72338912123a67330d723fdbb564d646c9f",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "489639febb8aba64115e19ffa11fed0601c30bab99022b8711dc1909ae80d34c",
        "index_record_sha256": "218d4017438679c4f956c1fcf095ec8ea2f1c5361e38649782c53e22fbc6a154",
        "cksum": "1e28d1d997f585e54aebc3f97d39e72338912123a67330d723fdbb564d646c9f",
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
      "sha256": "d94d6e62b2046eb5e08fa7ab5c25915e4e104f2111541b34d99aa165c2eb4714",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "3493b157b6f881363cfa0274bcfb11fa9c592ddcba79e188dd7499fe70054c20",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "37f85384d35aa686dd66b332fe940cf581f6414b1adf5d981751e3a6b2c058e2",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "README.md",
      "sha256": "05ba6a5490962c4df45b78e9ad928a29dd5c3fad749284d5b812ca7e765feb6d",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/enter.rs",
      "sha256": "ae3a196c0b32cf256e474a988b9fd9749fa645163b66915fbc894bea8e2efb56",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/local_pool.rs",
      "sha256": "ca73c98585f94c88228c147ad9e20db9e391cb507f47145851e9739b8f5fd628",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/thread_pool.rs",
      "sha256": "6fec12d757b73a5dad7f1c711cf947de2968a8b2d9d549bfad0a3921e77d5fdc",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "df07963835f3e7cb6cbae22dd88e91fca4cfb8a5fec5db15079cd8a006af429b",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "034652e42b5b7d0c31477ee3f112adad11c24c3fe4b2d851d528f1815ea45d5a",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "add5d253f32e3e9274f76db43072276bf39fe819d84a2d7498fac806a131cb80",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "7ff53eac06b5931596cb2a3c19b3e9a7b21b5415d35ee8f09e2ac3ca08edc29f",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "README.md",
      "sha256": "c245377709f81e8d92d5e162c3b28131294c0c0452bd69bee8df68777d23b2ac",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/enter.rs",
      "sha256": "babf753bade20f98931303797848466875f2f352c66b02ce7c5e1ddd568c13d1",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/local_pool.rs",
      "sha256": "eea2419bda86cb6c6e117d599859e29cde7e07bdfebbbcd2d0565cfa5246023b",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/thread_pool.rs",
      "sha256": "82d49ecea6da5eec3bef3f08c138a8248327bb26b1af709b0a47d42134fc4620",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/unpark_mutex.rs",
      "sha256": "ac2b4083561807be8f42e211f7e91a0aa0bb6238df5d3b2cb9712d62bb886cee",
      "scope": "whole target file, unchanged context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "df07963835f3e7cb6cbae22dd88e91fca4cfb8a5fec5db15079cd8a006af429b",
    "Cargo.lock": "034652e42b5b7d0c31477ee3f112adad11c24c3fe4b2d851d528f1815ea45d5a",
    "Cargo.toml": "add5d253f32e3e9274f76db43072276bf39fe819d84a2d7498fac806a131cb80",
    "Cargo.toml.orig": "7ff53eac06b5931596cb2a3c19b3e9a7b21b5415d35ee8f09e2ac3ca08edc29f",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "c245377709f81e8d92d5e162c3b28131294c0c0452bd69bee8df68777d23b2ac",
    "benches/thread_notify.rs": "e601968527bee85766f32d2d11de5ed8f6b4bd5a29989b5c369a52bd3cd3d024",
    "src/enter.rs": "babf753bade20f98931303797848466875f2f352c66b02ce7c5e1ddd568c13d1",
    "src/lib.rs": "c72ebbb09e231175d1e8286ed258207ee384454436cc2261a712d208abef60c3",
    "src/local_pool.rs": "eea2419bda86cb6c6e117d599859e29cde7e07bdfebbbcd2d0565cfa5246023b",
    "src/thread_pool.rs": "82d49ecea6da5eec3bef3f08c138a8248327bb26b1af709b0a47d42134fc4620",
    "src/unpark_mutex.rs": "ac2b4083561807be8f42e211f7e91a0aa0bb6238df5d3b2cb9712d62bb886cee",
    "tests/local_pool.rs": "f14fd260fd445c3b043f9a1bf73f149d5bb6867a84748be05bf7c68c93c7408c"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "d94d6e62b2046eb5e08fa7ab5c25915e4e104f2111541b34d99aa165c2eb4714",
    "Cargo.toml": "3493b157b6f881363cfa0274bcfb11fa9c592ddcba79e188dd7499fe70054c20",
    "Cargo.toml.orig": "37f85384d35aa686dd66b332fe940cf581f6414b1adf5d981751e3a6b2c058e2",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "05ba6a5490962c4df45b78e9ad928a29dd5c3fad749284d5b812ca7e765feb6d",
    "benches/thread_notify.rs": "e601968527bee85766f32d2d11de5ed8f6b4bd5a29989b5c369a52bd3cd3d024",
    "src/enter.rs": "ae3a196c0b32cf256e474a988b9fd9749fa645163b66915fbc894bea8e2efb56",
    "src/lib.rs": "c72ebbb09e231175d1e8286ed258207ee384454436cc2261a712d208abef60c3",
    "src/local_pool.rs": "ca73c98585f94c88228c147ad9e20db9e391cb507f47145851e9739b8f5fd628",
    "src/thread_pool.rs": "6fec12d757b73a5dad7f1c711cf947de2968a8b2d9d549bfad0a3921e77d5fdc",
    "src/unpark_mutex.rs": "ac2b4083561807be8f42e211f7e91a0aa0bb6238df5d3b2cb9712d62bb886cee",
    "tests/local_pool.rs": "f14fd260fd445c3b043f9a1bf73f149d5bb6867a84748be05bf7c68c93c7408c"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/src/local_pool.rs",
      "sha256": "eea2419bda86cb6c6e117d599859e29cde7e07bdfebbbcd2d0565cfa5246023b"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/src/enter.rs",
      "sha256": "babf753bade20f98931303797848466875f2f352c66b02ce7c5e1ddd568c13d1"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/src/thread_pool.rs",
      "sha256": "82d49ecea6da5eec3bef3f08c138a8248327bb26b1af709b0a47d42134fc4620"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/Cargo.toml",
      "sha256": "add5d253f32e3e9274f76db43072276bf39fe819d84a2d7498fac806a131cb80"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/Cargo.toml.orig",
      "sha256": "7ff53eac06b5931596cb2a3c19b3e9a7b21b5415d35ee8f09e2ac3ca08edc29f"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-executor-0.3.32/scratch/target/Cargo.lock",
      "sha256": "034652e42b5b7d0c31477ee3f112adad11c24c3fe4b2d851d528f1815ea45d5a"
    }
  ],
  "external_interface_context": [
    {
      "path": "crates.io/pin-utils-0.1.0/src/stack_pin.rs",
      "sha256": "b79661dc21f5ab923d75e3ddb52d43faf68fba7c7d592ec1929d1fd5275d70d8",
      "scope": "lines 14-25: base stack-pin macro; archive 8b870d8c151b6f2fb93e84a13146138f05d02ed11c7e7c54f8826aaaf7c9f184 matches registry index"
    },
    {
      "archive": "futures-util-0.3.31.crate",
      "archive_sha256": "9fa08315bb612088cc391249efdc3bc77536f16c91f6cf495e6fbe85b20a4a81",
      "member": "src/lib.rs",
      "member_sha256": "4d754ec8fb62b2e445a1fddfdd1aaa1cfe5d90b316e59a6005a4424cc4daa1f0",
      "scope": "line 25, old macro re-export; archive matches registry index"
    }
  ],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "a8d5516d20094126bc98140d00185f454af17391a0aae8b3a43693b4151b783e",
  "primary_report_sha256": "90d5a9014f2cfe13ae971d46bd45d4cb2adb58c7ceb9699562981aeb5dcd1d8b",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-executor 0.3.32: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `baf29c38818342a3b26b5b923639e7b1f4a61fc5e76102d4b1981c6dc7a7579d` matches the product-lock and registry-index checksums; base `0.3.31` archive `1e28d1d997f585e54aebc3f97d39e72338912123a67330d723fdbb564d646c9f` matches its registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of the trusted safe-to-deploy baseline links. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `a8d5516d20094126bc98140d00185f454af17391a0aae8b3a43693b4151b783e`; its changed-file set matches the frozen queue. `read-files.json` lists reviewed changed files, context and externally read dependency-interface source with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/enter.rs:4,55-78` changes the per-thread `Cell(false)` initializer to a const initializer; enter, reentrancy rejection and drop reset remain unchanged. `src/local_pool.rs:157-171,314-316` replace `pin_mut!` with standard `pin!`, holding a locally pinned future for the same polling scope. The old macro moves into a shadowed binding and uses `Pin::new_unchecked`; the new standard macro supplies a `Pin<&mut T>` and prevents moving a non-Unpin value while the local pin is live ([Rust pin documentation](https://doc.rust-lang.org/std/pin/macro.pin.html)).
- `src/thread_pool.rs:187-205,262-286` uses `thread::available_parallelism().map_or(1, ...)` rather than `max(1, num_cpus::get())`. It still selects a nonzero default and creates that many threads; the actual estimate may differ with platform/resource limits and errors fall back to one ([Rust documentation](https://doc.rust-lang.org/std/thread/fn.available_parallelism.html)). The thread name format rewrite is equivalent for the same prefix/index. Existing unsafe task ownership/state logic at 309-338 and unchanged `src/unpark_mutex.rs:10-26,56-135` are inherited from the base, not changed by this diff; no formal concurrency proof is claimed.
- Manifest and README raise MSRV 1.56 to 1.71; features retain default std, but thread-pool no longer enables num_cpus, which is removed. futures-core/task/util minimum versions rise to 0.3.32 and each needs separate auditing. `Cargo.toml:38-69` retains explicit library, same `build=false`, and std feature propagation. The new package-local lock records its independent dependency resolution, while the frozen product lock selects futures 0.3.32.

## Criteria and boundaries

Every authenticated baseline link explicitly carries safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer’s judgment the delta preserves those criteria. Changed default thread count may affect performance/capacity under container and OS limits. Dependency code and existing unsafe concurrency state machine remain separately scoped; no full platform or workload tests were run. Dependent crates must satisfy their own audits. This static review is neither formal verification nor cross-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.futures-executor]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.31 -> 0.3.32"
notes = "Full delta reviewed: standard local pinning, const TLS and available_parallelism fallback; default worker count may change; dependencies separately reviewed."
```


## Independent cross-review

Complete 0.3.31 -> 0.3.32 delta read including manifests, standalone lock and sources. src/local_pool.rs:157-171 changes pin_mut!(future) to std::pin::pin!(future); both locally pin the owned future for the polling scope, so its address is not moved during polling. The unchanged wake loop at 85-102 consumes an atomic wake flag before park and relies on the thread unpark token, preserving the existing wake handoff. src/enter.rs changes TLS initialization to const without changing per-thread Cell state. src/thread_pool.rs:190-193 changes default workers from num_cpus::get().max(1) to available_parallelism().map_or(1, |p| p.get()); the nonzero API and fallback keep at least one worker, though actual parallelism/resource scheduling can differ by OS. Manifest removes num_cpus, raises futures dependency minima to 0.3.32 and MSRV 1.56 -> 1.71; dependencies need their own review. Rust standard documentation describes local pinning at https://doc.rust-lang.org/std/pin/macro.pin.html. No exhaustive concurrency interleaving or platform tests claimed; bytecode-alliance full 0.3.31 is the authenticated safe-to-deploy baseline.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
