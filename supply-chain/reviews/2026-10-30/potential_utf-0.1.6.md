# Source review: potential_utf 0.1.2 -> 0.1.6

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.1.6",
    "archive_sha256": "d83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "39c11a0376c38501484426f880e4c67d2c6bb234afffcf43444adc0686eccd1e",
        "index_record_sha256": "0245a57b74ef6810a9c79a27e448c019308a6fb62413f7ac6b5e8a0fc8e70432",
        "cksum": "d83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "39c11a0376c38501484426f880e4c67d2c6bb234afffcf43444adc0686eccd1e",
        "index_record_sha256": "0245a57b74ef6810a9c79a27e448c019308a6fb62413f7ac6b5e8a0fc8e70432",
        "cksum": "d83eb9bc6d8e5cf568e7a1101d60ee05e81ed50ea106026f3d18deeb046d7661",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.1.2",
    "archive_sha256": "e5a7c30837279ca13e7c867e9e40053bc68740f988cb07f7ca6df43cc734b585",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "39c11a0376c38501484426f880e4c67d2c6bb234afffcf43444adc0686eccd1e",
        "index_record_sha256": "4749a2f482a68fa18edeb145bbff3647583826b7ec163ba3909bc522c1092e1f",
        "cksum": "e5a7c30837279ca13e7c867e9e40053bc68740f988cb07f7ca6df43cc734b585",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "39c11a0376c38501484426f880e4c67d2c6bb234afffcf43444adc0686eccd1e",
        "index_record_sha256": "4749a2f482a68fa18edeb145bbff3647583826b7ec163ba3909bc522c1092e1f",
        "cksum": "e5a7c30837279ca13e7c867e9e40053bc68740f988cb07f7ca6df43cc734b585",
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
        "version": "0.1.0",
        "notes": "Contains a handful of lines of from-UTF8 unsafety and some `repr(transparent)` casting unsafety. Reasonably well commented, could do with listing invariants explicitly.",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    },
    {
      "source": "google",
      "url": "https://raw.githubusercontent.com/google/rust-crate-audits/8cec794c38751495cf7a6c083220c207af1303d6/audits.toml",
      "source_sha256": "eb1c1bf1ee616471bfb56965eea325815ee41b5e3fcf5d12537dff2791160bbd",
      "original_record": {
        "who": "Manish Goregaokar <manishearth@google.com>",
        "criteria": [
          "safe-to-deploy",
          "crypto-safe",
          "ub-risk-2"
        ],
        "delta": "0.1.0 -> 0.1.2",
        "notes": "Addition of safe comparison APIs since last audit",
        "aggregated-from": "https://chromium.googlesource.com/chromium/src/+/main/third_party/rust/chromium_crates_io/supply-chain/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "cbcfa67b4dbafcd2cbdd43af7ee6a87db37dbb2088bdf9cdb5ae54905892722a"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": ".cargo_vcs_info.json",
      "sha256": "2ecaa8b9aa724092db7157db87ab3ad5cbecbb163d4ab8f384b033fb43a6ccca"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "79dcdab66c87309641bfc8e01f682d1b37b6b2dd85c273ea43a2279b3171e681"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.lock",
      "sha256": "8a67dddb2f78f42f5b53e8e32c248c507fa97ef8c742213a7a76d88bd55b8a6e"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "82fd80e6633b773f2c8c72016c7966f4dec32f36a27c6ae634490cfec34e0445"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml",
      "sha256": "62824fe5126abde181302cc0e4054ab6939355645ab8f12dd939ca1105604560"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "7226d58f0906a4dd5a6d57fa83028e569f61a241f3dc8bd67735e3563e2c69b2"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "Cargo.toml.orig",
      "sha256": "6361ac3442ab63dadc8adb8bcf97d5684b945b52dd12d52b6df525130aa83b4f"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "README.md",
      "sha256": "693ab95a7920db4ca6655b352efe33cfe5ee42792369f591257a3f5af735e918"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "README.md",
      "sha256": "c077e4d9e1438c188f0f8319b41376784562725e80bac136a3ca82062ac15b45"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "58fe437a809c113ce98e30c92ed4ccf9a78a31374e033bf56bb3384d76a0827d"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/lib.rs",
      "sha256": "afc7aba42182c6f287db27de8607b504d26fc33186dbd14349d6b8a851bb93c5"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/uchar.rs",
      "sha256": "157d4dd70360083d9d3e2f3f67d004ce282bcec0f378608dfac33835ffaf0e95"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/uchar.rs",
      "sha256": "fbab90a9c43d53cfcfba7f572adc71e2685642b00f671499e4619371ff2d2fc4"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/ustr.rs",
      "sha256": "3b9cbec56c5cfa233335220e459e9be1c3ef48e18ee07691c4bf3e29152a5e70"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/ustr.rs",
      "sha256": "bf592b05a373e7649b8cd6e88cddde247c674e72622fe73effdc41252366a33a"
    },
    {
      "side": "base",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/writeable.rs",
      "sha256": "9c381775e40a47db29f0601b6090897b5e649d9294ef74f11ac76dbf42c6448c"
    },
    {
      "side": "target",
      "coverage": "all changed lines and context in full.diff",
      "path": "src/writeable.rs",
      "sha256": "99856d0fffc27e142f0b7d8452ad119c7ebfaf08147881ee507b3be06828f11b"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "2ecaa8b9aa724092db7157db87ab3ad5cbecbb163d4ab8f384b033fb43a6ccca",
    "Cargo.lock": "8a67dddb2f78f42f5b53e8e32c248c507fa97ef8c742213a7a76d88bd55b8a6e",
    "Cargo.toml": "62824fe5126abde181302cc0e4054ab6939355645ab8f12dd939ca1105604560",
    "Cargo.toml.orig": "6361ac3442ab63dadc8adb8bcf97d5684b945b52dd12d52b6df525130aa83b4f",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "c077e4d9e1438c188f0f8319b41376784562725e80bac136a3ca82062ac15b45",
    "src/lib.rs": "afc7aba42182c6f287db27de8607b504d26fc33186dbd14349d6b8a851bb93c5",
    "src/uchar.rs": "fbab90a9c43d53cfcfba7f572adc71e2685642b00f671499e4619371ff2d2fc4",
    "src/ustr.rs": "bf592b05a373e7649b8cd6e88cddde247c674e72622fe73effdc41252366a33a",
    "src/writeable.rs": "99856d0fffc27e142f0b7d8452ad119c7ebfaf08147881ee507b3be06828f11b"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "cbcfa67b4dbafcd2cbdd43af7ee6a87db37dbb2088bdf9cdb5ae54905892722a",
    "Cargo.lock": "79dcdab66c87309641bfc8e01f682d1b37b6b2dd85c273ea43a2279b3171e681",
    "Cargo.toml": "82fd80e6633b773f2c8c72016c7966f4dec32f36a27c6ae634490cfec34e0445",
    "Cargo.toml.orig": "7226d58f0906a4dd5a6d57fa83028e569f61a241f3dc8bd67735e3563e2c69b2",
    "LICENSE": "f367c1b8e1aa262435251e442901da4607b4650e0e63a026f5044473ecfb90f2",
    "README.md": "693ab95a7920db4ca6655b352efe33cfe5ee42792369f591257a3f5af735e918",
    "src/lib.rs": "58fe437a809c113ce98e30c92ed4ccf9a78a31374e033bf56bb3384d76a0827d",
    "src/uchar.rs": "157d4dd70360083d9d3e2f3f67d004ce282bcec0f378608dfac33835ffaf0e95",
    "src/ustr.rs": "3b9cbec56c5cfa233335220e459e9be1c3ef48e18ee07691c4bf3e29152a5e70",
    "src/writeable.rs": "9c381775e40a47db29f0601b6090897b5e649d9294ef74f11ac76dbf42c6448c"
  },
  "independent_context_files": [
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/Cargo.toml",
      "sha256": "62824fe5126abde181302cc0e4054ab6939355645ab8f12dd939ca1105604560",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/Cargo.toml.orig",
      "sha256": "6361ac3442ab63dadc8adb8bcf97d5684b945b52dd12d52b6df525130aa83b4f",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/src/lib.rs",
      "sha256": "afc7aba42182c6f287db27de8607b504d26fc33186dbd14349d6b8a851bb93c5",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/src/ustr.rs",
      "sha256": "bf592b05a373e7649b8cd6e88cddde247c674e72622fe73effdc41252366a33a",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/src/uchar.rs",
      "sha256": "fbab90a9c43d53cfcfba7f572adc71e2685642b00f671499e4619371ff2d2fc4",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/scratch/potential_utf-0.1.6/target/src/writeable.rs",
      "sha256": "99856d0fffc27e142f0b7d8452ad119c7ebfaf08147881ee507b3be06828f11b",
      "inspection": "relevant lines/byte arrays and changed regions"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/potential_utf-0.1.6/full.diff",
      "sha256": "a85fd9efd2f7ad0293fe57195736c61d929e28e8f444ef4c00dbe0958421d5ad",
      "inspection": "all changed hunks and resources; independently reconstructed against archive bytes"
    },
    {
      "path": "validation/gates-fix-20261030/source-audits/lane-b-001/potential_utf-0.1.6/read-files.json",
      "sha256": "e5988de5da168c74975a0770ee9242329e690a092d9561f4026cf70f301e6d7c",
      "inspection": "independently checked against archive and changed-member coverage"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "a85fd9efd2f7ad0293fe57195736c61d929e28e8f444ef4c00dbe0958421d5ad",
  "primary_report_sha256": "f28da765bdb8b8b5de93bc8b6467a7178528dac369dc95ae29ac9bef55737708",
  "cross_report_sha256": "d06dadbc06d6ef08ce1028464b9e9ba8b5df27653e60140f8225cfe7933a0165"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# potential_utf-0.1.6: delta 0.1.2 → 0.1.6

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review)

Base chain: google:0.1.0 → google:0.1.0 -> 0.1.2. `inventory.json` preserves original records, source attribution and both archive checksums; `preflight.json` records source commits, blobs, SHA-256 and frozen lock. Every record explicitly contains safe-to-deploy and the path is contiguous.

- `src/ustr.rs:44-47,62-64,73-90,262-265,280-293`: transparent unsized wrappers have identical [u8]/[u16] layout, alignment and slice metadata; reference pointer casts retain the original borrow/length. Box conversion transfers the original allocation once through `into_raw`/`from_raw`, with same layout, allocator, and metadata. All byte sequences remain valid *PotentialUtf8* values; the cast does not assert UTF-8. New UTF-16 iterator uses `decode_utf16` with replacement for unpaired surrogates.
- `src/ustr.rs:203-255` and `src/uchar.rs` replace serde with serde_core; serialization still validates UTF-8/scalars, while binary deserialization deliberately accepts arbitrary bytes into the unvalidated wrapper. `src/writeable.rs:19-45` unchanged UTF-8 error offsets bound the unchecked slices; the delta removes an optimized `try_write_to_string` override, relying on the separate writeable trait's default. This can change allocation/formatting behavior and needs independent `writeable` review; it does not establish a formal UTF proof.
- `Cargo.toml` changes default to `alloc`, forwards `writeable/alloc` (thereby enabling the optional dependency) and `serde_core?/alloc`, replaces serde with serde_core, raises MSRV 1.81→1.82, and updates writeable/zerovec. Features and dependencies are separate audit obligations. Include list mentions build.rs but build=false and archive has no build.rs. Remaining changed files are metadata/lints/README; no C/asm/binary or new system-command path.

All changed hunks of every added/deleted/changed file were read in `full.diff` (SHA-256 `a85fd9efd2f7ad0293fe57195736c61d929e28e8f444ef4c00dbe0958421d5ad`), with six lines of context; `read-files.json` lists source pairs and their hashes. Unchanged files rely on the cited base. No formal proof, platform execution or recursive dependency audit is asserted.

Proposed record for independent review (not registered):

```toml
[[audits.potential_utf]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.1.2 -> 0.1.6"
```


## Independent cross-review

src/ustr.rs:44-47,62-90,262-293 has transparent [u8]/[u16] wrappers; pointer casts preserve slice metadata and Box into_raw/from_raw transfers the same allocation and layout once. UTF-16 chars uses safe decoding with replacement. src/writeable.rs:15-45 keeps bounded UTF-8 error-offset handling and removes its optimized string override; locked writeable 0.6.4 default at try_writeable.rs:207-225 uses length hint then try_write_to, yielding allocated best-effort output and the reported error. Serialization validates UTF-8/scalars; binary input may intentionally remain unvalidated. Cargo default now enables alloc (and writeable/alloc), switches serde to serde_core, and raises MSRV 1.81→1.82; these dependencies are separately gated. Google chain reaches 0.1.2.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
