# Source review: doc-comment 0.3.4

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-run`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.4",
    "archive_sha256": "780955b8b195a21ab8e4ac6b60dd1dbdcec1dc6c51c0617964b08c81785e12c9",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "6b2e4e085295455cc56df8ea0ce384588ee2a6beffcf417d58d04d50209f5b18",
        "index_record_sha256": "bb8844f3401d3639adafcf40aae43e72202f6f2aa392e15b94105684b8941ea9",
        "cksum": "780955b8b195a21ab8e4ac6b60dd1dbdcec1dc6c51c0617964b08c81785e12c9",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "6b2e4e085295455cc56df8ea0ce384588ee2a6beffcf417d58d04d50209f5b18",
        "index_record_sha256": "bb8844f3401d3639adafcf40aae43e72202f6f2aa392e15b94105684b8941ea9",
        "cksum": "780955b8b195a21ab8e4ac6b60dd1dbdcec1dc6c51c0617964b08c81785e12c9",
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
      "sha256": "f4d2ac4fd220b6503ad10acda02ced69c1c7bb743d02dc680aeb148d3331af11"
    },
    {
      "side": "target",
      "path": "build.rs",
      "sha256": "a342cd0a760b7e04b13406c5de82a9b6b39d9b8495a274c2f78d56d676aeca3a"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "ebaeb30654425b2afb0784da24a9d9744ed3eee49551acca55ed64499ef248c0"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "26bd77e04fe18a6ca7c271ded11759ff13392656f4be9b02cfa014ba7423ff4f"
    },
    {
      "side": "target",
      "path": "README.md",
      "sha256": "2276983b74b66c2fddbb111fce544bbd3f17a4da76452f2d581d047255fb35be"
    },
    {
      "side": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "5f20ad0bc9a0e2e633fde86feb1f18a312a4bb1c7c461e67ee58ca55a48ff69b",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": ".github/FUNDING.yml",
      "sha256": "237279a75de9c30b58d2fb24212c415da7df51f4c7ed4b65f72bcf5446da5079",
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
      "path": ".travis.yml",
      "sha256": "5aad7ac9774c47d1df640b89b52a90fc45d881da2275c7a20a7b1a42bb4f450c",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "Cargo.lock",
      "sha256": "81e65c3c88d9db260367464656edddd3505fee4ccab878fab0d2cee7eacc44e1",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "LICENSE",
      "sha256": "7a9b7dd9d68943a39bbf3ea97f37b3c7b257168ecbaa9e00606f64e6ff61fe35",
      "coverage": "presence and hash; license text"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "5f20ad0bc9a0e2e633fde86feb1f18a312a4bb1c7c461e67ee58ca55a48ff69b",
    ".github/FUNDING.yml": "237279a75de9c30b58d2fb24212c415da7df51f4c7ed4b65f72bcf5446da5079",
    ".gitignore": "944bf600c6230664922a011cbca026699969f2f89f6c7ff689835836ccd7b1de",
    ".travis.yml": "5aad7ac9774c47d1df640b89b52a90fc45d881da2275c7a20a7b1a42bb4f450c",
    "Cargo.lock": "81e65c3c88d9db260367464656edddd3505fee4ccab878fab0d2cee7eacc44e1",
    "Cargo.toml": "ebaeb30654425b2afb0784da24a9d9744ed3eee49551acca55ed64499ef248c0",
    "Cargo.toml.orig": "26bd77e04fe18a6ca7c271ded11759ff13392656f4be9b02cfa014ba7423ff4f",
    "LICENSE": "7a9b7dd9d68943a39bbf3ea97f37b3c7b257168ecbaa9e00606f64e6ff61fe35",
    "README.md": "2276983b74b66c2fddbb111fce544bbd3f17a4da76452f2d581d047255fb35be",
    "build.rs": "a342cd0a760b7e04b13406c5de82a9b6b39d9b8495a274c2f78d56d676aeca3a",
    "src/lib.rs": "f4d2ac4fd220b6503ad10acda02ced69c1c7bb743d02dc680aeb148d3331af11"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": null,
  "primary_report_sha256": "99fc9fd73c2b9272a0ad93150ae61e586083e7779b7b9a8b1147818b8997ba88",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# doc-comment 0.3.4 source review

Target archive SHA-256 `780955b8b195a21ab8e4ac6b60dd1dbdcec1dc6c51c0617964b08c81785e12c9`, independently matched to frozen Cargo.lock and registry index; safe-member and full inventory in `../provenance.json`.
Required `safe-to-run`. Reviewer `Codex (GPT-6.1 Terra; AI-assisted source review)`.

## Read files and SHA-256

- `.cargo_vcs_info.json` `5f20ad0bc9a0e2e633fde86feb1f18a312a4bb1c7c461e67ee58ca55a48ff69b`
- `Cargo.toml` `ebaeb30654425b2afb0784da24a9d9744ed3eee49551acca55ed64499ef248c0`
- `Cargo.toml.orig` `26bd77e04fe18a6ca7c271ded11759ff13392656f4be9b02cfa014ba7423ff4f`
- `Cargo.lock` `81e65c3c88d9db260367464656edddd3505fee4ccab878fab0d2cee7eacc44e1`
- `README.md` `2276983b74b66c2fddbb111fce544bbd3f17a4da76452f2d581d047255fb35be`
- `.travis.yml` `5aad7ac9774c47d1df640b89b52a90fc45d881da2275c7a20a7b1a42bb4f450c`
- `build.rs` `a342cd0a760b7e04b13406c5de82a9b6b39d9b8495a274c2f78d56d676aeca3a`
- `src/lib.rs` `f4d2ac4fd220b6503ad10acda02ced69c1c7bb743d02dc680aeb148d3331af11`

## Analysis

Full source and manifest/build path read without relying on optional old base. Cargo.toml:12-37 declares build.rs, no dependencies, features no_core and old_macros, and explicit library. CI metadata mentions rustc 1.24.1 but normalized manifest has no rust-version; no complete compiler matrix was run. No native/C/asm/binary members.

build.rs:1-24 invokes `rustc --version` via Command and inherited PATH, reads stdout, parses the first version components, and emits only `cargo:rustc-cfg=feature="old_macros"` for 1.x minor <30. Spawn errors, malformed UTF-8 or unexpected version produce no directive. It makes no shell, network, filesystem mutation, environment mutation or key access; trusting executable resolution is an ordinary toolchain/PATH boundary. src/lib.rs:172-181 expands caller-supplied doc expression into a doc attribute and optional item; doctest! lines 199-234 passes caller-selected path to include_str! for documentation, including an old-macro compatibility form. This can read caller-requested local files at compilation and rustdoc can execute code blocks when the caller elects to run doctests; it introduces neither an implicit path nor runtime capabilities. no_core feature requires a nightly compiler. There is no unsafe/FFI or shared mutable state.

Criteria: for this graph the required safe-to-run is met by the source reviewed; expected compiler execution and caller-directed documentation tests are disclosed. No claim about safe-to-deploy is needed or made. Static review only; no all-toolchain execution or recursive dependency certification.

## Proposed audit (not registered)

```toml
[[audits.doc-comment]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-run"
version = "0.3.4"
```


## Independent cross-review

build.rs:3-24 invokes rustc --version through inherited PATH, checks output format and toggles old_macros only for Rust 1.<30; errors return without cfg. The compiler executable and PATH are trusted build inputs, and this is a real process launch. src/lib.rs:172-181,199-234 passes caller-supplied documentation, items and include_str path to rustdoc; doctests may execute caller-supplied code when requested. No implicit secret path, unsafe code, or network operation was found. Agrees with queue-required safe-to-run, not a new safe-to-deploy claim.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
