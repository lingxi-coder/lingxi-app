# Source review: futures-io 0.3.31 -> 0.3.32

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.32",
    "archive_sha256": "cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "d3a080bf9043ea714ceb0b130679c8b6edf022d32aa904e28ec1e11eb88c6f4e",
        "index_record_sha256": "f5cb0b853b2870e4458fa2a8744bcaa46e832973a196f4d24b328644d4f9f5fd",
        "cksum": "cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.31",
    "archive_sha256": "9e5c1b78ca4aae1ac06c48a526a655760685149f0d465d21f37abfe57ce075c6",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "d3a080bf9043ea714ceb0b130679c8b6edf022d32aa904e28ec1e11eb88c6f4e",
        "index_record_sha256": "2a5c0735804af40ebce7afb5df99d4d334131ce30c759e03559e0073a5cc046e",
        "cksum": "9e5c1b78ca4aae1ac06c48a526a655760685149f0d465d21f37abfe57ce075c6",
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
      "sha256": "13f596c5f04f4e4f1f8f4a2a4e27d8c8ba549e025637a54443a141e18112883f",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "1b91c3dcf4888590e4e0499bbc7f261d97278782875032bef60952b4f47c5872",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "202efc67b9e9675b909e80557678c10a715553c7ce7faebcf604f0af8ebd8675",
      "scope": "full changed file",
      "method": "whole old manifest or full delta against the reviewed target"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "214325c00ab5e42ed636dbc9f0bab92395a01dcd01240dc54d3c2460d3e49849",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.lock",
      "sha256": "effb2b02e6667d5fab0e2a8615748faca8856cbc5e49948e57fe23d3d46a4a5c",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "25f7523cb891a7491bb35aa4787212209d2ee2b8845b6243b9e7c82e0d1ac6c9",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "74e47285baa89d734998ae95f56fd21c484fda6e5549780eff50f42ff1f103e5",
      "scope": "full changed file",
      "method": "complete changed target file; baseline derived by full diff where not printed separately"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "ff0a57a4c0eea4b7677404cdca237729f0af9c5dfc9539d23583a71ac7e9cc42",
      "scope": "lines 1-115 for trait and default I/O context",
      "method": "source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "214325c00ab5e42ed636dbc9f0bab92395a01dcd01240dc54d3c2460d3e49849",
    "Cargo.lock": "effb2b02e6667d5fab0e2a8615748faca8856cbc5e49948e57fe23d3d46a4a5c",
    "Cargo.toml": "25f7523cb891a7491bb35aa4787212209d2ee2b8845b6243b9e7c82e0d1ac6c9",
    "Cargo.toml.orig": "74e47285baa89d734998ae95f56fd21c484fda6e5549780eff50f42ff1f103e5",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "575430be5c47352d85f36b44dcc2c2851a6a19e2384593415c4af22c6654cee7",
    "src/lib.rs": "ff0a57a4c0eea4b7677404cdca237729f0af9c5dfc9539d23583a71ac7e9cc42"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "13f596c5f04f4e4f1f8f4a2a4e27d8c8ba549e025637a54443a141e18112883f",
    "Cargo.toml": "1b91c3dcf4888590e4e0499bbc7f261d97278782875032bef60952b4f47c5872",
    "Cargo.toml.orig": "202efc67b9e9675b909e80557678c10a715553c7ce7faebcf604f0af8ebd8675",
    "LICENSE-APACHE": "275c491d6d1160553c32fd6127061d7f9606c3ea25abfad6ca3f6ed088785427",
    "LICENSE-MIT": "6652c868f35dfe5e8ef636810a4e576b9d663f3a17fb0f5613ad73583e1b88fd",
    "README.md": "575430be5c47352d85f36b44dcc2c2851a6a19e2384593415c4af22c6654cee7",
    "src/lib.rs": "ff0a57a4c0eea4b7677404cdca237729f0af9c5dfc9539d23583a71ac7e9cc42"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-io-0.3.32/scratch/target/src/lib.rs",
      "sha256": "ff0a57a4c0eea4b7677404cdca237729f0af9c5dfc9539d23583a71ac7e9cc42"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-io-0.3.32/scratch/target/Cargo.toml",
      "sha256": "25f7523cb891a7491bb35aa4787212209d2ee2b8845b6243b9e7c82e0d1ac6c9"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/futures-io-0.3.32/scratch/target/Cargo.lock",
      "sha256": "effb2b02e6667d5fab0e2a8615748faca8856cbc5e49948e57fe23d3d46a4a5c"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "ded610bce1244e63c8ef5643c071338e105fd659d90a50f713c6c8fa87abb175",
  "primary_report_sha256": "7a63b6cb5d6f45b6db7faafb0ac4d5fcfc5b677b909e14a74e2d9a21ddc1b3b3",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# futures-io 0.3.32: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. No audit record has been registered. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review).

## Identity and evidence

- Frozen batch: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; lane queue: `1f7f8c3bc014da44f795bbb3984f7ae191c9a1269cfdeafffbd1dc9a1123ea72`; product Cargo.lock: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
- Target archive `cecba35d7ad927e23624b22ad55235f2239cfa44fd10428eecbeba6d6a717718` matches the frozen Cargo.lock and its crate-index checksum; base `0.3.31` archive `9e5c1b78ca4aae1ac06c48a526a655760685149f0d465d21f37abfe57ce075c6` matches its crate-index checksum. Both normalized manifests agree with their name/version.
- `verification.json` records original archive and index paths, index-file hashes, every member SHA-256 and mode, and the original records of the trusted full/delta chain ending at `0.3.31`. The source snapshots match their fixed SHA-256 and Git blob IDs. Every path is within the expected package prefix, unique and regular, with no links or binary members.
- `full.diff` is the complete byte-driven textual package diff (ded610bce1244e63c8ef5643c071338e105fd659d90a50f713c6c8fa87abb175); its changed-file set equals the frozen queue. `read-files.json` records the reviewed changed files and contextual source with their hashes. Unchanged files are identified by hashing; this is a delta review, not an assertion that the full base was independently re-audited.

## Analysis

- `Cargo.toml:12-47`: MSRV remains 1.36 and the sole explicit library remains `src/lib.rs`; `build = false` remains. The newly explicit `autolib = false` disables automatic library discovery, but the explicit `[lib]` points to the same file. The empty dependencies section and `default = ["std"]`, `std = []`, `unstable = []` are unchanged apart from table order.
- `Cargo.toml.orig:1-23` adds an MSRV synchronization comment and changes package version. The new package-local `Cargo.lock:1-6` only names this package; `.cargo_vcs_info.json` changes the revision. The single source file and other package files are byte-identical. `src/lib.rs:22-102` still gates the I/O traits on `std` and delegates vectored reads to the implementor; the change does not add an I/O implementation or a new capability.

## Criteria and boundaries

The checked baseline path has explicit `safe-to-deploy` on every full/delta link, which also implies `safe-to-run`; see [cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html) and [delta semantics](https://mozilla.github.io/cargo-vet/audit-entries.html). The above source and manifest delta preserves those criteria in this reviewer’s judgment. The I/O operations are supplied by callers implementing the inherited traits; the reviewed delta adds no process, filesystem, network, environment, unsafe, or credential code. No feature behavior changed.

Dependent crates need their own assessments; this review does not recursively certify them. It is static source review, not formal verification or cross-platform runtime acceptance. No product, lock, registry, or formal audit store was modified.

## Proposed record, for root review only

```toml
[[audits.futures-io]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.31 -> 0.3.32"
notes = "Full package delta and manifests reviewed; source bytes and dependency/feature behavior unchanged."
```


## Independent cross-review

Complete 0.3.31 -> 0.3.32 package delta has unchanged Rust source bytes, no dependency, build or binary changes; Cargo.lock adds self only, normalized manifest sets autolib=false while explicitly retaining src/lib.rs, and reordered std/unstable features are unchanged. MSRV remains 1.36. No new IO implementation or filesystem/FFI/env capabilities: interface contracts and any implementation remain separately reviewed. Authenticated bytecode-alliance full 0.3.31 covers deploy; no formal runtime tests claimed.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
