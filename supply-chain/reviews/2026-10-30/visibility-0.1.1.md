# Source review: visibility 0.1.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: full package.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.1.1",
    "archive_sha256": "d674d135b4a8c1d7e813e2f8d1c9a58308aee4a680323066025e53132218bd91",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "ca65bbc7e9bcde9a273a3722303b78c104288b764f0f912d7ba9bebb4856e6a2",
        "index_record_sha256": "205e79ba91151cc71a3ece44da3eed53c7c5f04fe88297fc4839ca00fdd7599e",
        "cksum": "d674d135b4a8c1d7e813e2f8d1c9a58308aee4a680323066025e53132218bd91",
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
      "sha256": "7e25371c90526aa1501abc223020c9f6f77fbb549c877e798b372511f3b5017a"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "a58164491fa17b6958c8909a6a44c6d8ad21a77abe8aa7ab7816db5727c82e5d"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "ec2980a81721c6f412cee50d5fd9ccf189d5cfaecdabf65b4f4ce07463ff5706"
    },
    {
      "side": "target",
      "path": "tests/downstream.rs",
      "sha256": "b086a5aef7ef83ddd06f5669f4332ce50a008718fdfe1c8b3cb6697c5352960d"
    },
    {
      "side": "target",
      "path": "README.md",
      "sha256": "de05b79b1ca846eb98531ef818a167ea87ac1a3aec42d690e2bee97eb04f5e51"
    },
    {
      "side": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "c0e64b39ea512bcbe57b77db49b52f6e5c0d2192462ff7396aa6da0e2ed47f7d",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": ".github/workflows/CI.yml",
      "sha256": "2e2a00bdb78b27173e92aa80feb8c3a50f9a2593f6d64e6a3d586cd210b48b0a",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": ".github/workflows/future-proof.yml",
      "sha256": "82f4a2949f8c35b0c7038cf9c4350279c60b25bcba663db3e86bda06310aab32",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": ".gitignore",
      "sha256": "44c92e3a70ad3307b7056871c2bdb096d8bfa9373f5bf06a79bb6324a20ff2fb",
      "coverage": "metadata or ancillary text read"
    },
    {
      "side": "target",
      "path": "LICENSE",
      "sha256": "b72b2d40a984de67680d4b874dea846f5e389599bbe495b22f63e87c6f1d5f0c",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "LICENSE-APACHE",
      "sha256": "50d26cd90660ea5dfca5db399824028bca4eda4ee5be5d50a1b0be573936cabe",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "LICENSE-MIT",
      "sha256": "b405b5dbae5d4473f5f932af2b5d47df6114036a0b1382d9f6c411a62b71a547",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "LICENSE-ZLIB",
      "sha256": "23f99ac423f5950bbd814d05e77e5e7be5183903133f3938be2a313cb8070205",
      "coverage": "presence and hash; license text"
    },
    {
      "side": "target",
      "path": "rust-toolchain",
      "sha256": "19f1c71cabd3cb062544023af771fbb2883524775fdaed6ca5855bae359b7b9c",
      "coverage": "metadata or ancillary text read"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "c0e64b39ea512bcbe57b77db49b52f6e5c0d2192462ff7396aa6da0e2ed47f7d",
    ".github/workflows/CI.yml": "2e2a00bdb78b27173e92aa80feb8c3a50f9a2593f6d64e6a3d586cd210b48b0a",
    ".github/workflows/future-proof.yml": "82f4a2949f8c35b0c7038cf9c4350279c60b25bcba663db3e86bda06310aab32",
    ".gitignore": "44c92e3a70ad3307b7056871c2bdb096d8bfa9373f5bf06a79bb6324a20ff2fb",
    "Cargo.toml": "a58164491fa17b6958c8909a6a44c6d8ad21a77abe8aa7ab7816db5727c82e5d",
    "Cargo.toml.orig": "ec2980a81721c6f412cee50d5fd9ccf189d5cfaecdabf65b4f4ce07463ff5706",
    "LICENSE": "b72b2d40a984de67680d4b874dea846f5e389599bbe495b22f63e87c6f1d5f0c",
    "LICENSE-APACHE": "50d26cd90660ea5dfca5db399824028bca4eda4ee5be5d50a1b0be573936cabe",
    "LICENSE-MIT": "b405b5dbae5d4473f5f932af2b5d47df6114036a0b1382d9f6c411a62b71a547",
    "LICENSE-ZLIB": "23f99ac423f5950bbd814d05e77e5e7be5183903133f3938be2a313cb8070205",
    "README.md": "de05b79b1ca846eb98531ef818a167ea87ac1a3aec42d690e2bee97eb04f5e51",
    "rust-toolchain": "19f1c71cabd3cb062544023af771fbb2883524775fdaed6ca5855bae359b7b9c",
    "src/lib.rs": "7e25371c90526aa1501abc223020c9f6f77fbb549c877e798b372511f3b5017a",
    "tests/downstream.rs": "b086a5aef7ef83ddd06f5669f4332ce50a008718fdfe1c8b3cb6697c5352960d"
  },
  "base_inventory_sha256": {},
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": null,
  "primary_report_sha256": "3ede7b6c73e177ce104cc12180e2d77cc817bd6e1767d7fb348397e500bc6ec9",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# visibility 0.1.1 source review

Archive SHA-256: `d674d135b4a8c1d7e813e2f8d1c9a58308aee4a680323066025e53132218bd91`; checked independently against frozen Cargo.lock and crates.io index; complete safe-member and per-file inventory in `../provenance.json`.

Required: `safe-to-deploy`. Reviewer: `Codex (GPT-6.1 Terra; AI-assisted source review)`.

## Read files and SHA-256

- `.cargo_vcs_info.json` `c0e64b39ea512bcbe57b77db49b52f6e5c0d2192462ff7396aa6da0e2ed47f7d`
- `Cargo.toml` `a58164491fa17b6958c8909a6a44c6d8ad21a77abe8aa7ab7816db5727c82e5d`
- `Cargo.toml.orig` `ec2980a81721c6f412cee50d5fd9ccf189d5cfaecdabf65b4f4ce07463ff5706`
- `src/lib.rs` `7e25371c90526aa1501abc223020c9f6f77fbb549c877e798b372511f3b5017a`
- `tests/downstream.rs` `b086a5aef7ef83ddd06f5669f4332ce50a008718fdfe1c8b3cb6697c5352960d`
- `README.md` `de05b79b1ca846eb98531ef818a167ea87ac1a3aec42d690e2bee97eb04f5e51`
- `rust-toolchain` `19f1c71cabd3cb062544023af771fbb2883524775fdaed6ca5855bae359b7b9c`
- `.github/workflows/CI.yml` `2e2a00bdb78b27173e92aa80feb8c3a50f9a2593f6d64e6a3d586cd210b48b0a`
- `.github/workflows/future-proof.yml` `82f4a2949f8c35b0c7038cf9c4350279c60b25bcba663db3e86bda06310aab32`

## Analysis

Full source review. Cargo.toml:12-17,40-60 declares a proc macro, `build = false`, nightly empty feature and proc-macro2/quote/syn full dependency; no local build script, native files or generated binary are present. The README include at src/lib.rs:3-5 is documentation text, not generated code. README records MSRV 1.56; dependencies are separate audit scopes.

src/lib.rs:1 forbids unsafe. Lines 52-83 parse an explicit visibility and Rust item with syn; supported item variants have only their `vis` field replaced and are emitted as tokens. Unsupported variants return compile_error instead of emitting an unmodified or unexpected item. The caller deliberately chooses the expanded visibility; this can expose an item by design and is not a hidden environment, filesystem, network, or process capability. No unsafe/FFI, global mutable state or concurrency is in this macro. The only subprocess is the test at tests/downstream.rs:2-10: it calls the Cargo executable with fixed `test --doc --features integration-tests` in `downstream_crate`, which is absent in the published archive, so that packaged test may fail; ordinary compilation and expansion do not execute it. CI YAML is not part of the dependent Cargo build.

Criteria: the compile-time macro emits the caller-selected visibility and compile errors on unsupported syntax; no serious vulnerability was identified in this package's reviewed source under safe-to-deploy, which includes safe-to-run. Caller choice of visibility matters. Dependencies and all-platform compiler/expansion behavior were not recursively certified or run. Static review only.

## Proposed full audit (not registered)

```toml
[[audits.visibility]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
version = "0.1.1"
```


## Independent cross-review

src/lib.rs:52-83 parses caller-selected visibility and item, changes only a supported Item vis field, returns compile_error for unmatched item, then re-emits tokens; src/lib.rs:1 forbids unsafe. Caller can intentionally widen visibility. The only local process spawn is test-only tests/downstream.rs:2-10, targeting an absent fixture; proc-macro dependencies are separately gated. Manifest declares proc-macro, no build script. No package-local reason to reject safe-to-deploy.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
