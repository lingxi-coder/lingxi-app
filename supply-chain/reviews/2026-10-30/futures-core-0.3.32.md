# Source review: futures-core 0.3.34 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "efd49b47d990ff3d5f4759803b18ac1d16b6f277d4e202b362c5ede397f550aa",
        "index_record_sha256": "486d52c43373f4a89d27c64ad3dd8ac48d7d1007050cb9a86557569e4def381c",
        "cksum": "7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.34",
    "archive_sha256": "92d699e522242e69e3003b94ecc1f960f3a5e015aa7c5d7486e65ad01dd94f5e",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "efd49b47d990ff3d5f4759803b18ac1d16b6f277d4e202b362c5ede397f550aa",
        "index_record_sha256": "eee5c0c74d374523237ce09bab2704443441a5f9cb1aaf26074d85801f6ec675",
        "cksum": "92d699e522242e69e3003b94ecc1f960f3a5e015aa7c5d7486e65ad01dd94f5e",
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
        "who": "Pat Hickey <phickey@fastly.com>",
        "criteria": "safe-to-deploy",
        "version": "0.3.27",
        "notes": "Unsafe used to implement a concurrency primitive AtomicWaker. Well-commented and not obviously incorrect. Like my other audits of these concurrency primitives inside the futures family, I couldn't certify that it is correct without formal methods, but that is out of scope for this vetting."
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Mike Hommey <mh+mozilla@glandium.org>",
        "criteria": "safe-to-deploy",
        "delta": "0.3.27 -> 0.3.28",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla-firefox/firefox/main/supply-chain/audits.toml"
      }
    },
    {
      "source": "bytecode-alliance",
      "url": "https://raw.githubusercontent.com/bytecodealliance/wasmtime/04f0fcf70248909cc0b6a3ec1d406c7e2d543637/supply-chain/audits.toml",
      "source_sha256": "71f9f1e8a51d25708f2bb152f39e90a6bdc3d17d91d0f5df4cbc608cef465fd2",
      "original_record": {
        "who": "Pat Hickey <pat@moreproductive.org>",
        "criteria": "safe-to-deploy",
        "delta": "0.3.28 -> 0.3.31"
      }
    },
    {
      "source": "mozilla",
      "url": "https://raw.githubusercontent.com/mozilla/supply-chain/e6648794a1deb2821ddebd846564fd99f420b432/audits.toml",
      "source_sha256": "97ac6f1002cc4e32f9a71ce5e16d2962325bdf356519d66eb21be2dc4f2ce58d",
      "original_record": {
        "who": "Alex Franchuk <afranchuk@mozilla.com>",
        "criteria": "safe-to-deploy",
        "delta": "0.3.31 -> 0.3.34",
        "aggregated-from": "https://raw.githubusercontent.com/mozilla/cargo-vet/main/supply-chain/audits.toml"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "9e19550cc5c74b21e06edc8ac1935458e09de23b2dd45b1635c41075db33f951",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.lock",
      "sha256": "2d99d08db413bbac7697a71410aea129c4a5902896b115a4ec0547d20625dba2",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "fa88cec6d0c527df4ff51ad034de09d3c552f6174ab8e4aa83cfaeacc36c4c7d",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "b862d981fc4ed113c5cc319643da53ff4ff2b6f30141b124c4e6cefba8283b5f",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "69b2d02e5db8288df704d7a276ef0bfb27017f5ba1657efddd33135076f82ba0",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "fbeac5791a2d97ac81be504c9436354ec6f4d2fa43172945f9bbac1097d84faf",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "8e1fc1b21e82013b3accf43c8fc1bcdddeb3d41c810110fb83a61558a49742c3",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "be1d2616172fb5ae584491a8cf34dd5e4f330a70a1056286cbde6bf4f358c9f8",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "e545004177a7cd13257a3a562d2d44a5e0cff45687fc912b69e3d510fa397396",
      "scope": "whole file",
      "method": "source context"
    },
    {
      "role": "target",
      "path": "src/task/__internal/atomic_waker.rs",
      "sha256": "67d3e3177332e95696bc727420beadc1af44b5c96da78ac8f97f9f5bae4fb143",
      "scope": "whole file",
      "method": "source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "69b2d02e5db8288df704d7a276ef0bfb27017f5ba1657efddd33135076f82ba0",
    "Cargo.lock": "fbeac5791a2d97ac81be504c9436354ec6f4d2fa43172945f9bbac1097d84faf",
    "Cargo.toml": "8e1fc1b21e82013b3accf43c8fc1bcdddeb3d41c810110fb83a61558a49742c3",
    "Cargo.toml.orig": "be1d2616172fb5ae584491a8cf34dd5e4f330a70a1056286cbde6bf4f358c9f8",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "e8258273fed6f1796485777655118f2369fd3f000191e9d8cdbd10bf052946a9",
    "src/future.rs": "bdbe034548271aef0c3dd8a6d087a5861e5920848c9e33a8949dbc40407c0ca7",
    "src/lib.rs": "e545004177a7cd13257a3a562d2d44a5e0cff45687fc912b69e3d510fa397396",
    "src/stream.rs": "11f0b4360287dd870c1b674db84f2452ddc38fbaf475cca27d374b65211af72d",
    "src/task/__internal/atomic_waker.rs": "67d3e3177332e95696bc727420beadc1af44b5c96da78ac8f97f9f5bae4fb143",
    "src/task/__internal/mod.rs": "1cc15fd61942a29ea558c5f4d5782e46adcfd914cab82be084a6882fa9afc122",
    "src/task/mod.rs": "e213602a2fe5ae78ad5f1ca20e6d32dcbab17aba5b6b072fb927a72da99b4a11",
    "src/task/poll.rs": "3732f2e489b107ef76ba0cd9b30efd197d445dcea67f2c65783a388893f310b6"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "9e19550cc5c74b21e06edc8ac1935458e09de23b2dd45b1635c41075db33f951",
    "Cargo.lock": "2d99d08db413bbac7697a71410aea129c4a5902896b115a4ec0547d20625dba2",
    "Cargo.toml": "fa88cec6d0c527df4ff51ad034de09d3c552f6174ab8e4aa83cfaeacc36c4c7d",
    "Cargo.toml.orig": "b862d981fc4ed113c5cc319643da53ff4ff2b6f30141b124c4e6cefba8283b5f",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "e8258273fed6f1796485777655118f2369fd3f000191e9d8cdbd10bf052946a9",
    "src/future.rs": "bdbe034548271aef0c3dd8a6d087a5861e5920848c9e33a8949dbc40407c0ca7",
    "src/lib.rs": "e545004177a7cd13257a3a562d2d44a5e0cff45687fc912b69e3d510fa397396",
    "src/stream.rs": "11f0b4360287dd870c1b674db84f2452ddc38fbaf475cca27d374b65211af72d",
    "src/task/__internal/atomic_waker.rs": "67d3e3177332e95696bc727420beadc1af44b5c96da78ac8f97f9f5bae4fb143",
    "src/task/__internal/mod.rs": "1cc15fd61942a29ea558c5f4d5782e46adcfd914cab82be084a6882fa9afc122",
    "src/task/mod.rs": "e213602a2fe5ae78ad5f1ca20e6d32dcbab17aba5b6b072fb927a72da99b4a11",
    "src/task/poll.rs": "3732f2e489b107ef76ba0cd9b30efd197d445dcea67f2c65783a388893f310b6"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-core-0.3.32/scratch/target/src/task/__internal/atomic_waker.rs",
      "sha256": "67d3e3177332e95696bc727420beadc1af44b5c96da78ac8f97f9f5bae4fb143"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-core-0.3.32/scratch/target/Cargo.toml",
      "sha256": "8e1fc1b21e82013b3accf43c8fc1bcdddeb3d41c810110fb83a61558a49742c3"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-core-0.3.32/scratch/target/Cargo.lock",
      "sha256": "fbeac5791a2d97ac81be504c9436354ec6f4d2fa43172945f9bbac1097d84faf"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "7e26b3f453ad17dd25172dd73d8bfa2623f11971df7108ad6b72ec5155f03128",
  "primary_report_sha256": "4c2920769fd04b3e9db8cd85e27a79a44a080fddb3ef1b95e771a7f119674c77",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-core 0.3.32: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. No audit record has been registered. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review).

## Identity and evidence

- Frozen batch: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; lane queue: `1f7f8c3bc014da44f795bbb3984f7ae191c9a1269cfdeafffbd1dc9a1123ea72`; product Cargo.lock: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
- Target archive `7e3450815272ef58cec6d564423f6e755e25379b217b0bc688e295ba24df6b1d` matches the frozen Cargo.lock and its crate-index checksum; base `0.3.34` archive `92d699e522242e69e3003b94ecc1f960f3a5e015aa7c5d7486e65ad01dd94f5e` matches its crate-index checksum. Both normalized manifests agree with their name/version.
- `verification.json` records original archive and index paths, index-file hashes, every member SHA-256 and mode, and the original records of the trusted full/delta chain ending at `0.3.34`. The source snapshots match their fixed SHA-256 and Git blob IDs. Every path is within the expected package prefix, unique and regular, with no links or binary members.
- `full.diff` is the complete byte-driven textual package diff (7e26b3f453ad17dd25172dd73d8bfa2623f11971df7108ad6b72ec5155f03128); its changed-file set equals the frozen queue. `read-files.json` records the reviewed changed files and contextual source with their hashes. Unchanged files are identified by hashing; this is a delta review, not an assertion that the full base was independently re-audited.

## Analysis

- `Cargo.toml:12-53` and `Cargo.toml.orig:1-25`: only package version changes. The target retains MSRV 1.36, identical features (`std -> alloc`, optional `portable-atomic` with `require-cas`), the same explicit library and `build = false`. All seven `.rs` files are byte-identical between the verified archives, so the base's source and include set are preserved.
- `Cargo.lock:3-16`: the package's own lock shifts portable-atomic 1.13.1 to 1.11.1. This is not the product workspace lock: its separately frozen lock selects 1.15.0. The manifest requirement did not change. Review of portable-atomic itself is separate; no claim about its safety follows here. `.cargo_vcs_info.json` changes source revision identity only.
- Context `src/task/__internal/atomic_waker.rs:96-125,266-329,378-423`: unchanged `UnsafeCell<Option<Waker>>` is accessed only after the atomic state wins the registration/wake critical section; the contended wake path takes the waker and resets the state. The unsafe Send/Sync assertions and memory ordering are inherited unchanged. This comparison is not a formal concurrency proof, and the originally accepted base's caveat about formal proof remains applicable.

## Criteria and boundaries

The checked baseline path has explicit `safe-to-deploy` on every full/delta link, which also implies `safe-to-run`; see [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) and [delta semantics](https://mozilla.github.io/cargo-vet/audit-entries.html). The above source and manifest delta preserves those criteria in this reviewer’s judgment. The same concurrency and unsafe implementation as the authenticated 0.3.34 base is inherited; portable-atomic must satisfy its own audit. No new file, native code, build script, network, process, filesystem, environment, or credential access is introduced in the delta.

Dependent crates need their own assessments; this review does not recursively certify them. It is static source review, not formal verification or cross-platform runtime acceptance. No product, lock, registry, or formal audit store was modified.

## Proposed record, for root review only

```toml
[[audits.futures-core]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.34 -> 0.3.32"
notes = "Full package delta and manifests reviewed; executable source identical; atomic-waker concurrency inherited and not formally proven; portable-atomic reviewed separately."
```


## Independent cross-review

Reverse 0.3.34 -> 0.3.32 changes only VCS id, package versions and standalone Cargo.lock portable-atomic 1.13.1 -> 1.11.1. Independently reconstructed diff covers all archive members; every .rs file, module/include and feature/dependency specification is byte-identical, so this reverse edge does not remove any crate-local source safety fix relative to 0.3.34. Frozen product Cargo.lock chooses portable-atomic 1.15.0; standalone package lock is different and portable-atomic itself needs independent audit for any actual resolution. AtomicWaker context src/task/__internal/atomic_waker.rs:266-329 claims REGISTERING/WAKING ownership, CAS/AcqRel, and contended wake handoff; unchanged, no new unsafe code, not formally verified. 0.3.27 full -> 0.3.28 -> 0.3.31 -> 0.3.34 has explicit safe-to-deploy at each independently authenticated edge. No claim that an unrelated dependency has no regressions.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
