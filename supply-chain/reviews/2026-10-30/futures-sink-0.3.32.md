# Source review: futures-sink 0.3.31 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5d8e794aa845e4c7093089bda91a7d17f7321a9fcae4950004533607985384fa",
        "index_record_sha256": "1bc527406303b535ec9f9ba034391450c47a41847b9e27cacee05f8622f2a1b6",
        "cksum": "c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.31",
    "archive_sha256": "e575fab7d1e0dcb8d0c7bcf9a63ee213816ab51902e6d244a95819acacf1d4f7",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "5d8e794aa845e4c7093089bda91a7d17f7321a9fcae4950004533607985384fa",
        "index_record_sha256": "d359c7c33501b307e34005894ca110b1824e5694c1db0aba7959751d28e217e0",
        "cksum": "e575fab7d1e0dcb8d0c7bcf9a63ee213816ab51902e6d244a95819acacf1d4f7",
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
        "version": "0.3.27"
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
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "4f1c26a0d1452481048a9fa9091078ea4bc8bbc78a31ae5460ff8e1d92eaad3c",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "112aafe39494ee4cf0529fa06b0264d96cde567f4c2ea5d20539cd97b6f537bd",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "c27355f40979b69c53c35d7ff74549f9f9d8e28fa6597cd7798f9d115dca0fb8",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "a1a85f2388fa9e27886f7e44272ac38aefadb80c852e406d50437ca37b22e3c8",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "79ec71ba9b58857e26e335ad8e352359ec5e5a2a172b18e34b6d66d567727e17",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "735810b8578352403f118d3e0bd227e42775a4fae0e1e65cb1c793df8d17b2f2",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "59100b395194de757c7e78158789af49622527f4b8dac80b6633d311e625aeef",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "ab6425171a64636d45e40cebab75a48feec17c5dd8a305b3a48c4b7345df3ce5",
      "scope": "whole file",
      "method": "source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "a1a85f2388fa9e27886f7e44272ac38aefadb80c852e406d50437ca37b22e3c8",
    "Cargo.lock": "79ec71ba9b58857e26e335ad8e352359ec5e5a2a172b18e34b6d66d567727e17",
    "Cargo.toml": "735810b8578352403f118d3e0bd227e42775a4fae0e1e65cb1c793df8d17b2f2",
    "Cargo.toml.orig": "59100b395194de757c7e78158789af49622527f4b8dac80b6633d311e625aeef",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "a509e1ce84f285190130def6d2b9e3861988f9be725f7697f09fba347601d86f",
    "src/lib.rs": "ab6425171a64636d45e40cebab75a48feec17c5dd8a305b3a48c4b7345df3ce5"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "4f1c26a0d1452481048a9fa9091078ea4bc8bbc78a31ae5460ff8e1d92eaad3c",
    "Cargo.toml": "112aafe39494ee4cf0529fa06b0264d96cde567f4c2ea5d20539cd97b6f537bd",
    "Cargo.toml.orig": "c27355f40979b69c53c35d7ff74549f9f9d8e28fa6597cd7798f9d115dca0fb8",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "a509e1ce84f285190130def6d2b9e3861988f9be725f7697f09fba347601d86f",
    "src/lib.rs": "ab6425171a64636d45e40cebab75a48feec17c5dd8a305b3a48c4b7345df3ce5"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-sink-0.3.32/scratch/target/src/lib.rs",
      "sha256": "ab6425171a64636d45e40cebab75a48feec17c5dd8a305b3a48c4b7345df3ce5"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-sink-0.3.32/scratch/target/Cargo.toml",
      "sha256": "735810b8578352403f118d3e0bd227e42775a4fae0e1e65cb1c793df8d17b2f2"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-sink-0.3.32/scratch/target/Cargo.lock",
      "sha256": "79ec71ba9b58857e26e335ad8e352359ec5e5a2a172b18e34b6d66d567727e17"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "e41f3e669c00e89ecac55b82372a9607e2427d16eb26751191f4a8e4049d06cc",
  "primary_report_sha256": "b61d9456f88e4ffbf6a064a84eb10bc7574fd9ddcdf392182eb7496f8790edc5",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-sink 0.3.32: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. No audit record has been registered. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review).

## Identity and evidence

- Frozen batch: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; lane queue: `1f7f8c3bc014da44f795bbb3984f7ae191c9a1269cfdeafffbd1dc9a1123ea72`; product Cargo.lock: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
- Target archive `c39754e157331b013978ec91992bde1ac089843443c49cbc7f46150b0fad0893` matches the frozen Cargo.lock and its crate-index checksum; base `0.3.31` archive `e575fab7d1e0dcb8d0c7bcf9a63ee213816ab51902e6d244a95819acacf1d4f7` matches its crate-index checksum. Both normalized manifests agree with their name/version.
- `verification.json` records original archive and index paths, index-file hashes, every member SHA-256 and mode, and the original records of the trusted full/delta chain ending at `0.3.31`. The source snapshots match their fixed SHA-256 and Git blob IDs. Every path is within the expected package prefix, unique and regular, with no links or binary members.
- `full.diff` is the complete byte-driven textual package diff (e41f3e669c00e89ecac55b82372a9607e2427d16eb26751191f4a8e4049d06cc); its changed-file set equals the frozen queue. `read-files.json` records the reviewed changed files and contextual source with their hashes. Unchanged files are identified by hashing; this is a delta review, not an assertion that the full base was independently re-audited.

## Analysis

- `Cargo.toml:12-43` retains MSRV 1.36, `build = false`, the explicit `src/lib.rs` library and empty dependency table. New `autolib = false` disables auto-discovery but leaves the declared library intact. `alloc`, `std -> alloc` and default `std` are unchanged and merely reordered. `Cargo.toml.orig:1-19` changes the version and adds an MSRV comment.
- A package-local `Cargo.lock:1-6` names only this package, and `.cargo_vcs_info.json` changes revision. The only Rust file and all other non-manifest package content are byte-identical to the authenticated base.
- Context `src/lib.rs:119-160,163-200`: delegation keeps `Unpin` bounds where needed; existing `get_unchecked_mut` for pinned Vec/VecDeque mutates the container without promising structural pinning of elements. The unsafe implementation and async `Sink` contract are unchanged, rather than newly introduced by this delta.

## Criteria and boundaries

The checked baseline path has explicit `safe-to-deploy` on every full/delta link, which also implies `safe-to-run`; see [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) and [delta semantics](https://mozilla.github.io/cargo-vet/audit-entries.html). The above source and manifest delta preserves those criteria in this reviewer’s judgment. The two existing unsafe container accesses are inherited unchanged, with the same pinning assumptions; no new filesystem, process, network, env or credential capability appears in the delta.

Dependent crates need their own assessments; this review does not recursively certify them. It is static source review, not formal verification or cross-platform runtime acceptance. No product, lock, registry, or formal audit store was modified.

## Proposed record, for root review only

```toml
[[audits.futures-sink]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.31 -> 0.3.32"
notes = "Full package delta and manifests reviewed; source bytes unchanged, including inherited pinned container operations."
```


## Independent cross-review

Complete 0.3.31 -> 0.3.32 package delta changes only VCS id, self-only Cargo.lock, explicit autolib=false while preserving lib path, version and reordering identical alloc/std/default features; source unchanged. In inherited src/lib.rs:119-160 pinned wrappers delegate without moving the pinned target; 163-177 Vec mutation uses get_unchecked_mut and push, unchanged from baseline. No new unsafe/FFI or ambient IO in this delta. Verified deploy chain bytecode-alliance 0.3.27 full -> mozilla 0.3.28 -> bytecode-alliance 0.3.31; dependencies and base unsafe soundness not re-certified.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
