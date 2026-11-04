# Source review: colorchoice 1.0.5

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "1.0.5",
    "archive_sha256": "1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "0e3383a397661d11e2b336f6bfd62a8fa7cafc248b3cdbd73b145fca0add8220",
        "index_record_sha256": "73dc4ab69799429c9674c75bcd1ef0b9a0395a807d2a26dc8fdaf7a00ebdb5ad",
        "cksum": "1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "0e3383a397661d11e2b336f6bfd62a8fa7cafc248b3cdbd73b145fca0add8220",
        "index_record_sha256": "73dc4ab69799429c9674c75bcd1ef0b9a0395a807d2a26dc8fdaf7a00ebdb5ad",
        "cksum": "1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570",
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
      "sha256": "9777ea67e426637377c2cb035ff41d2a4dd2a5bd6796d86e2afbfe70a8ba342d"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "403ea406945c6d350b0a0f6b5c01d5bee1e59d522059884b7f15a0b44c64b579"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "954a31b1e693564aa84c38fbe0f71d1f7739aed2ab193b98f7966255bb48db14"
    },
    {
      "side": "target",
      "path": "README.md",
      "sha256": "fc9b47302f5a3244b4e447e5bf6f86a35c64a16bd1abef8c27f31c4b5abac7a2"
    },
    {
      "side": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "2c8cf91d264ad4efb1d6d5c46c07e7467c64156f727dd347d53274c9a63a3ae3",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "Cargo.lock",
      "sha256": "6ea206462f9bd8281d69ee7cc52c44c65feb425266e95c724a59dd57250afee7",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "LICENSE-APACHE",
      "sha256": "c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "LICENSE-MIT",
      "sha256": "6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6",
      "coverage": "presence and hash; license text"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "2c8cf91d264ad4efb1d6d5c46c07e7467c64156f727dd347d53274c9a63a3ae3",
    "Cargo.lock": "6ea206462f9bd8281d69ee7cc52c44c65feb425266e95c724a59dd57250afee7",
    "Cargo.toml": "403ea406945c6d350b0a0f6b5c01d5bee1e59d522059884b7f15a0b44c64b579",
    "Cargo.toml.orig": "954a31b1e693564aa84c38fbe0f71d1f7739aed2ab193b98f7966255bb48db14",
    "LICENSE-APACHE": "c6596eb7be8581c18be736c846fb9173b69eccf6ef94c5135893ec56bd92ba08",
    "LICENSE-MIT": "6efb0476a1cc085077ed49357026d8c173bf33017278ef440f222fb9cbcb66e6",
    "README.md": "fc9b47302f5a3244b4e447e5bf6f86a35c64a16bd1abef8c27f31c4b5abac7a2",
    "src/lib.rs": "9777ea67e426637377c2cb035ff41d2a4dd2a5bd6796d86e2afbfe70a8ba342d"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": null,
  "primary_report_sha256": "484fdfa020d412ed0466990c702db296e74a7f80fddda9850a6b52f217fccc0d",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# colorchoice 1.0.5 source review

Archive SHA-256: `1d07550c9036bf2ae0c684c4297d503f838287c83c53686d05370d0e139ae570`; checked independently against frozen Cargo.lock and crates.io index; complete safe-member and per-file inventory in `../provenance.json`.

Required: `safe-to-deploy`. Reviewer: `Codex (GPT-6.1 Terra; AI-assisted source review)`.

## Read files and SHA-256

- `.cargo_vcs_info.json` `2c8cf91d264ad4efb1d6d5c46c07e7467c64156f727dd347d53274c9a63a3ae3`
- `Cargo.toml` `403ea406945c6d350b0a0f6b5c01d5bee1e59d522059884b7f15a0b44c64b579`
- `Cargo.toml.orig` `954a31b1e693564aa84c38fbe0f71d1f7739aed2ab193b98f7966255bb48db14`
- `Cargo.lock` `6ea206462f9bd8281d69ee7cc52c44c65feb425266e95c724a59dd57250afee7`
- `src/lib.rs` `9777ea67e426637377c2cb035ff41d2a4dd2a5bd6796d86e2afbfe70a8ba342d`
- `README.md` `fc9b47302f5a3244b4e447e5bf6f86a35c64a16bd1abef8c27f31c4b5abac7a2`

## Analysis

Full source review. Cargo.toml:12-31 sets edition 2021, MSRV 1.66, `build = false`, and disables automatic targets; the declared library is src/lib.rs:88-90. `include` mentions build.rs but none is present and `build = false`; no dependencies, features, build script, generated native code or binary are in this archive. The package Cargo.lock contains only itself.

src/lib.rs:14-27 defines four choices; lines 29-38 expose get/set. USER uses AtomicUsize:41-58, initialized from Auto; all writes map a valid enum to 0..3 and SeqCst stores/loads, so racing calls yield a valid complete choice, not a data race or invalid discriminant. to_choice checks each integer and `expect` at 51-54 cannot fail through this safe API because no other write path exists. This is process-global state and last writer wins; it controls a color choice only. No unsafe, FFI, macro generation, process, filesystem, env, key or network access exists in executable source; the README doc-test include is test-only.

Criteria: no serious security vulnerability identified in reviewed source for safe-to-deploy, which includes safe-to-run. Global setting is intentional and affects all callers in the process. Static review, no cross-platform runtime testing; no dependency audit claimed.

## Proposed full audit (not registered)

```toml
[[audits.colorchoice]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
version = "1.0.5"
```


## Independent cross-review

src/lib.rs:14-27,41-76 represents the global value with an AtomicUsize; all safe stores originate in exhaustive enum-to-0..3 conversion, SeqCst loads map only those values back. No unchecked discriminant cast, FFI, subprocess or hidden IO exists. Last writer wins and affects process-wide color choice; the inclusion list mentions build.rs but build=false and none is shipped. Agrees with full safe-to-deploy.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
