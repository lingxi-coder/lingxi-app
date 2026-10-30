# Source review: shell-words 1.1.0 -> 1.1.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "1.1.1",
    "archive_sha256": "dc6fe69c597f9c37bfeeeeeb33da3530379845f10be461a66d16d03eca2ded77",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "fefc848953cf2f6f8172ee62d2cb2a413573e29b953e67698ade2cc747590535",
        "index_record_sha256": "253318ac44fe9e5d75f3094872d30b05ef60058654c161f7dc680a835b3af03b",
        "cksum": "dc6fe69c597f9c37bfeeeeeb33da3530379845f10be461a66d16d03eca2ded77",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "1.1.0",
    "archive_sha256": "24188a676b6ae68c3b2cb3a01be17fbf7240ce009799bb56d5b1409051e78fde",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "fefc848953cf2f6f8172ee62d2cb2a413573e29b953e67698ade2cc747590535",
        "index_record_sha256": "b525da0d89bb624ad473e5a1d5a272fee28a73c7e48c0dfe01a12fb25482f604",
        "cksum": "24188a676b6ae68c3b2cb3a01be17fbf7240ce009799bb56d5b1409051e78fde",
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
        "who": "George Burgess IV <gbiv@google.com>",
        "criteria": [
          "does-not-implement-crypto",
          "safe-to-deploy"
        ],
        "version": "1.1.0",
        "aggregated-from": "https://chromium.googlesource.com/chromiumos/third_party/rust_crates/+/refs/heads/main/cargo-vet/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "a4db7fbfca8e02aaadb5395672f82a74db4688d33e982084099183a25cf809b8",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "93a9e03b9ff9da01305caf81ce6d2d452b9d43d8247e5122437cb2804c2c6dc8",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "2b5ed7695dcc6f145118a6cf390a9cdbf24116ac9bce50f8229fb7d8d71e75b8",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "be96845d39315fc620e12abd42e29a593ec5d267b9a40462a15cf5be234d3149",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "8cbbc1f2892d50ca3bde0902247fbd22f9839bcf5dd3e00a6caba6a826a0f5f5",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "4af67b81b8a3c2736351535fc78a14b31d3a1919b56773d6efa1d27ba4058cb9",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "d018c5f50842202d615d71d5d909162410c6f4b38c0b4602c41a4f5bd7116fc1",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "04e120face6f0da5cd3eb7c7d9abbd3fb296bffef7cd59ffc3c7205be6756745",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "dabd20b4ebd5a4f248f98748346f6fe8fefd3c645f47eb6d91ed86ee5ac0180e",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "8cbbc1f2892d50ca3bde0902247fbd22f9839bcf5dd3e00a6caba6a826a0f5f5",
    ".gitignore": "6b937008e0a58a438c32574627db6961a4d2aff4bf5c703d4e45cb56ef3b168a",
    "Cargo.lock": "4af67b81b8a3c2736351535fc78a14b31d3a1919b56773d6efa1d27ba4058cb9",
    "Cargo.toml": "d018c5f50842202d615d71d5d909162410c6f4b38c0b4602c41a4f5bd7116fc1",
    "Cargo.toml.orig": "04e120face6f0da5cd3eb7c7d9abbd3fb296bffef7cd59ffc3c7205be6756745",
    "LICENSE-APACHE": "848d7bf14c618c39f1e29fcac2318a2fcc4254045ba7e2f6f5fa799e057225cb",
    "LICENSE-MIT": "f76b7831ee8f42d3ac38919101e4ceb9f56aeb7796cdfe8a5ef52e26acedcbbf",
    "README.md": "9afd45a0555ac08318b35b569d439e9aa3e9b15323622648bce72e9663e12453",
    "rustfmt.toml": "aeffdf96c9207bb6b1cb14c082f2f7c702653664c2f23d705b6a6b3de055c9d4",
    "src/lib.rs": "dabd20b4ebd5a4f248f98748346f6fe8fefd3c645f47eb6d91ed86ee5ac0180e"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "a4db7fbfca8e02aaadb5395672f82a74db4688d33e982084099183a25cf809b8",
    ".gitignore": "6b937008e0a58a438c32574627db6961a4d2aff4bf5c703d4e45cb56ef3b168a",
    "Cargo.toml": "93a9e03b9ff9da01305caf81ce6d2d452b9d43d8247e5122437cb2804c2c6dc8",
    "Cargo.toml.orig": "2b5ed7695dcc6f145118a6cf390a9cdbf24116ac9bce50f8229fb7d8d71e75b8",
    "LICENSE-APACHE": "848d7bf14c618c39f1e29fcac2318a2fcc4254045ba7e2f6f5fa799e057225cb",
    "LICENSE-MIT": "f76b7831ee8f42d3ac38919101e4ceb9f56aeb7796cdfe8a5ef52e26acedcbbf",
    "README.md": "9afd45a0555ac08318b35b569d439e9aa3e9b15323622648bce72e9663e12453",
    "rustfmt.toml": "aeffdf96c9207bb6b1cb14c082f2f7c702653664c2f23d705b6a6b3de055c9d4",
    "src/lib.rs": "be96845d39315fc620e12abd42e29a593ec5d267b9a40462a15cf5be234d3149"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/shell-words-1.1.1/scratch/target/src/lib.rs",
      "sha256": "dabd20b4ebd5a4f248f98748346f6fe8fefd3c645f47eb6d91ed86ee5ac0180e"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/shell-words-1.1.1/scratch/target/Cargo.toml",
      "sha256": "d018c5f50842202d615d71d5d909162410c6f4b38c0b4602c41a4f5bd7116fc1"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/shell-words-1.1.1/scratch/target/Cargo.lock",
      "sha256": "4af67b81b8a3c2736351535fc78a14b31d3a1919b56773d6efa1d27ba4058cb9"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "0bca64ef8dd8fa7bdb3e630a731f2778318777e53cda8001abe484e93e06e8be",
  "primary_report_sha256": "f760e147c6ccf03d4b79d8ebd116c264c67555cdd7bbae7b5cdb116a903b4169",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# shell-words 1.1.1: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. No audit record has been registered. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review).

## Identity and evidence

- Frozen batch: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; lane queue: `1f7f8c3bc014da44f795bbb3984f7ae191c9a1269cfdeafffbd1dc9a1123ea72`; product Cargo.lock: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
- Target archive `dc6fe69c597f9c37bfeeeeeb33da3530379845f10be461a66d16d03eca2ded77` matches the frozen Cargo.lock and its crate-index checksum; base `1.1.0` archive `24188a676b6ae68c3b2cb3a01be17fbf7240ce009799bb56d5b1409051e78fde` matches its crate-index checksum. Both normalized manifests agree with their name/version.
- `verification.json` records original archive and index paths, index-file hashes, every member SHA-256 and mode, and the original records of the trusted full/delta chain ending at `1.1.0`. The source snapshots match their fixed SHA-256 and Git blob IDs. Every path is within the expected package prefix, unique and regular, with no links or binary members.
- `full.diff` is the complete byte-driven textual package diff (0bca64ef8dd8fa7bdb3e630a731f2778318777e53cda8001abe484e93e06e8be); its changed-file set equals the frozen queue. `read-files.json` records the reviewed changed files and contextual source with their hashes. Unchanged files are identified by hashing; this is a delta review, not an assertion that the full base was independently re-audited.

## Analysis

- `src/lib.rs:240-275`: `escape_style` changes a U+02DC small tilde match to ASCII `~`. ASCII tilde now marks a word special; U+02DC is no longer unnecessarily marked on its own. `quote` at 282-300 wraps a special word in single quotes and escapes embedded single quotes; `join` at 334-346 uses that result. This prevents shell tilde expansion when callers use returned strings as shell words. `src/lib.rs:475-482` adds a matching `~root` assertion. `split` continues to implement its documented restricted tokenization rather than execute a shell (66-79,126-228).
- Target `Cargo.toml:12-40` explicitly disables auto-discovery/build and declares the same `src/lib.rs` library; `std` remains default and no dependencies are introduced. The original manifest changes only its package version. The new Cargo.lock v4 names only this crate and `.cargo_vcs_info.json` changes revision; no build script or added packaged source appears.
- `#![forbid(unsafe_code)]` is unchanged at `src/lib.rs:11-12`. Documented examples invoking `Command`/environment at 101-124 are no-run examples, not behavior added to the library. Shell dialect and caller execution context still matter; this is not a general proof for every shell invocation.

## Criteria and boundaries

The checked baseline path has explicit `safe-to-deploy` on every full/delta link, which also implies `safe-to-run`; see [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) and [delta semantics](https://mozilla.github.io/cargo-vet/audit-entries.html). The above source and manifest delta preserves those criteria in this reviewer’s judgment. No new process execution, network, filesystem, env or secrets handling in the library. The change improves ASCII tilde preservation; downstream callers remain responsible for selecting an appropriate shell and using the API as documented.

Dependent crates need their own assessments; this review does not recursively certify them. It is static source review, not formal verification or cross-platform runtime acceptance. No product, lock, registry, or formal audit store was modified.

Small isolated probe: `rustc 1.98.1 --test --cfg feature="std" scratch/target/src/lib.rs -o scratch/shell_words_unit`, then `scratch/shell_words_unit --exact tests::test_quote`: 1 passed; 12 filtered. One lifetime-syntax warning. This does not test other toolchains or shells.

## Proposed record, for root review only

```toml
[[audits.shell-words]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "1.1.0 -> 1.1.1"
notes = "Full package delta and quoting flow reviewed; ASCII tilde now quoted, U+02DC no longer specially quoted. Focused quote unit test passed."
```


## Independent cross-review

Full delta 1.1.0 -> 1.1.1; full delta includes manifests/added self lock and sole behavior edit src/lib.rs:241-299. Replacing U+02DC with ASCII ~ in escape_style causes ~root to enter Mixed and be single-quoted, stopping shell tilde expansion; Unicode U+02DC is no longer needlessly quoted and is not ASCII shell syntax. Quote path handles apostrophes by close/escape/reopen and newline via single quote; test adds ~root. Declared lib replaces autodiscovery and build=false matches existing absence of build script. No new shell process execution or unsafe introduced, use of resulting string still relies on Unix-shell parsing context; google full 1.1.0 safe-to-deploy authenticated.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
