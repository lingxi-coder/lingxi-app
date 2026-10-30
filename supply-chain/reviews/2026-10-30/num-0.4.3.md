# Source review: num 0.4.3

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.4.3",
    "archive_sha256": "35bd024e8b2ff75562e5f34e7f4905839deb4b22955ef5e73d2fea1b9813cb23",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "087bea42568b044660b2f880a9eef0382a0062c6f6ecb391a3e35a42e7bef019",
        "index_record_sha256": "4c9fada134d539d1223f9a1e1850bccdebf5774154a86544b4e380c9e4a6d6e4",
        "cksum": "35bd024e8b2ff75562e5f34e7f4905839deb4b22955ef5e73d2fea1b9813cb23",
        "yanked": false
      }
    ]
  },
  "base": null,
  "baseline_chain": [],
  "read_files": [
    {
      "side": "target",
      "path": "src/lib.rs",
      "sha256": "eb4d945da75ed1b5ed386bc8a8fa6a32e9ea3caf7167d1ea635cd5821b6862ac"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "d99a1aff5b981cc5842a82bd67905c030932f5b6167f9a5bc3c056d80d945189"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "15526a87c418bea9fa7333b776ae36ff53852c1e56d62ceba2fba87222469fd0"
    },
    {
      "side": "target",
      "path": "README.md",
      "sha256": "d3819dc286fd7ce24a46612df0b886509d6a1d1ee623d54e6df0150a26e24d5a"
    },
    {
      "side": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "a220620da990c2feb926bf56373617f5b29c62f400e288532262e284fee5bbd8",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": ".gitignore",
      "sha256": "944bf600c6230664922a011cbca026699969f2f89f6c7ff689835836ccd7b1de",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "LICENSE-APACHE",
      "sha256": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "LICENSE-MIT",
      "sha256": "6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "RELEASES.md",
      "sha256": "5a41c12b50d42c1cf05f66faeffb4e344cd58018d3d12ef56cb235617bba2254",
      "coverage": "metadata or ancillary text read"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "a220620da990c2feb926bf56373617f5b29c62f400e288532262e284fee5bbd8",
    ".gitignore": "944bf600c6230664922a011cbca026699969f2f89f6c7ff689835836ccd7b1de",
    "Cargo.toml": "d99a1aff5b981cc5842a82bd67905c030932f5b6167f9a5bc3c056d80d945189",
    "Cargo.toml.orig": "15526a87c418bea9fa7333b776ae36ff53852c1e56d62ceba2fba87222469fd0",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "6485b8ed310d3f0340bf1ad1f47645069ce4069dcc6bb46c7d5c6faf41de1fdb",
    "README.md": "d3819dc286fd7ce24a46612df0b886509d6a1d1ee623d54e6df0150a26e24d5a",
    "RELEASES.md": "5a41c12b50d42c1cf05f66faeffb4e344cd58018d3d12ef56cb235617bba2254",
    "src/lib.rs": "eb4d945da75ed1b5ed386bc8a8fa6a32e9ea3caf7167d1ea635cd5821b6862ac"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": null,
  "primary_report_sha256": "c40b8e5bcec0538b0295422ceb5306591372bfd9dfff4fb414e5cd91ec94e3ec",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# num 0.4.3 source review

Archive SHA-256: `35bd024e8b2ff75562e5f34e7f4905839deb4b22955ef5e73d2fea1b9813cb23`; checked independently against frozen Cargo.lock and crates.io index; complete safe-member and per-file inventory in `../provenance.json`.

Required: `safe-to-deploy`. Reviewer: `Codex (GPT-6.1 Terra; AI-assisted source review)`.

## Read files and SHA-256

- `.cargo_vcs_info.json` `a220620da990c2feb926bf56373617f5b29c62f400e288532262e284fee5bbd8`
- `Cargo.toml` `d99a1aff5b981cc5842a82bd67905c030932f5b6167f9a5bc3c056d80d945189`
- `Cargo.toml.orig` `15526a87c418bea9fa7333b776ae36ff53852c1e56d62ceba2fba87222469fd0`
- `src/lib.rs` `eb4d945da75ed1b5ed386bc8a8fa6a32e9ea3caf7167d1ea635cd5821b6862ac`
- `README.md` `d3819dc286fd7ce24a46612df0b886509d6a1d1ee623d54e6df0150a26e24d5a`
- `RELEASES.md` `5a41c12b50d42c1cf05f66faeffb4e344cd58018d3d12ef56cb235617bba2254`

## Analysis

Full source review (no candidate base needed). Cargo.toml:12-15 declares edition 2021/MSRV 1.60. Lines 50-76 declare num-bigint/complex/integer/iter/rational/traits; lines 80-109 select std, alloc, libm, rand and serde dependency features, including optional bigint. No build script or proc macro is declared or shipped. The feature choices can activate code in dependencies; their source is outside this audit.

src/lib.rs:59-60 is no_std; lines 62-108 conditionally re-export numeric types, traits and functions from those dependencies, with alloc/std gating bigint and libm/std gating Float. It defines no arithmetic, unsafe, FFI, native/binary include, macros, mutation, networking, files, environment, key handling or subprocesses of its own. Doctest sample lines 17-49 uses re-exported arbitrary-precision arithmetic; README confirms this is a meta-crate. No dependency functionality is asserted to be audited by this finding.

Criteria: this re-export crate's source presents no identified serious security vulnerability for safe-to-deploy (including safe-to-run), subject to separate review of each selected dependency and its features. Rust versions/feature matrices were not built. Static review only.

## Proposed full audit (not registered)

```toml
[[audits.num]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
version = "0.4.3"
```


## Independent cross-review

src/lib.rs:59-108 is a no_std reexport facade with bigint and floating-point exports gated by alloc/std/libm; it implements no numeric operation or unsafe path itself. Cargo.toml:50-109 selects numeric crates/features, including bigint via std/alloc/rand/serde, and no local build script exists in the verified archive. Numeric implementations and all enabled dependencies still need their own reviews. Agrees with package-local full safe-to-deploy.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
