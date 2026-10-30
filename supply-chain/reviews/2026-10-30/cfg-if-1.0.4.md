# Source review: cfg-if 1.0.0 -> 1.0.4

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "1.0.4",
    "archive_sha256": "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "8f6eaea0f7fc5f6848486cbdf55738698871d39e9120c4c1c68fe5c964696d7d",
        "index_record_sha256": "aaeec487cab298de772ad629cebb8dc4b456a0ddbbc14c20d612d23ae608110b",
        "cksum": "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "8d48ab1d1657f18075ece1b2e4d764975301df9756be764ddb91281815b5a00d",
        "index_record_sha256": "aaeec487cab298de772ad629cebb8dc4b456a0ddbbc14c20d612d23ae608110b",
        "cksum": "9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "1.0.0",
    "archive_sha256": "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "8f6eaea0f7fc5f6848486cbdf55738698871d39e9120c4c1c68fe5c964696d7d",
        "index_record_sha256": "02f86964d66f3e46ffc6e851d42013339c479167370367eca970884fcaaf6f25",
        "cksum": "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd",
        "yanked": false
      },
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "8d48ab1d1657f18075ece1b2e4d764975301df9756be764ddb91281815b5a00d",
        "index_record_sha256": "02f86964d66f3e46ffc6e851d42013339c479167370367eca970884fcaaf6f25",
        "cksum": "baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd",
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
        "who": "Alex Crichton <alex@alexcrichton.com>",
        "criteria": "safe-to-deploy",
        "version": "1.0.0",
        "notes": "I am the author of this crate."
      }
    }
  ],
  "read_files": [
    {
      "side": "base",
      "path": "src/lib.rs",
      "sha256": "54b0f108b0dc48a077c52c0bcd22b64ef4de083e5e2b7d405e50ae4d78224f1b",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "tests/xcrate.rs",
      "sha256": "c0734dae6e63beafcd60bf53546115a2320735b51035c9e2387fdf9301580934",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml",
      "sha256": "5b2a8f6e5256957c029cf3a8912d51438e7faa5891c5c102c312f6d4599c1f00",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "base",
      "path": "Cargo.toml.orig",
      "sha256": "90923641c7cd59e9f3df14ab37d39523d4a6e59cf02d7a6aae79abcaca07e3b9",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "src/lib.rs",
      "sha256": "c09723e0890d15810374009e96b20bf0eb2f65f383006516f34db36240835c85",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "tests/xcrate.rs",
      "sha256": "bcec148e69db81b1a618bdd6f96a25d9a0442e6ecc692fe28f1206d9bffc006a",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml",
      "sha256": "281d508beb1fe3927cf03d3f2f8c9a5117b1e4fe97ae21b9026cf318e8c35273",
      "coverage": "changed hunks and relevant source context"
    },
    {
      "side": "target",
      "path": "Cargo.toml.orig",
      "sha256": "5a17ee17da78f5179373b8324a1180e71efe2bcf3e3c9ca18c1bdb1e3faa9792",
      "coverage": "changed hunks and relevant source context"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "65840ba044457862e25b2d7d53f4a2de232adf933bd31aba8f2bd6a1f51a6881",
    ".github/dependabot.yml": "828e3ecefdc555a5210a5bdffd5621ef3625ceb35c7fc91a0b4faef6f9921b75",
    ".github/workflows/main.yaml": "6612a51b1f1479eabac7d3bd14aa609811d4afd2df2d454b9a1f6d6f3748f5b2",
    ".github/workflows/publish.yaml": "1417805078704eecbaeea8611c5a44df575bfe1908d6969d909224a6e5e26ca8",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    "CHANGELOG.md": "08ba7340057565b338afaa29b36bd2a1c46f5495b043bc49d12230a6a82d5f76",
    "Cargo.lock": "26922b9384045e5a3d496f21ec7c355da585d0caa1d13b887b634527d36fc450",
    "Cargo.toml": "281d508beb1fe3927cf03d3f2f8c9a5117b1e4fe97ae21b9026cf318e8c35273",
    "Cargo.toml.orig": "5a17ee17da78f5179373b8324a1180e71efe2bcf3e3c9ca18c1bdb1e3faa9792",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397",
    "README.md": "cd565d563a2c70d197bb6fee1678e122214e22af7bdb046b80f52c1d953cd72f",
    "src/lib.rs": "c09723e0890d15810374009e96b20bf0eb2f65f383006516f34db36240835c85",
    "tests/xcrate.rs": "bcec148e69db81b1a618bdd6f96a25d9a0442e6ecc692fe28f1206d9bffc006a"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "adc7a2b67bc53600a229c2892d592977aa4939434fb93f53b2e98799173a675c",
    ".github/workflows/main.yml": "27a418e9130e6462907cf246c5b71c9d51bc684873219c561011928dd829d445",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    "Cargo.toml": "5b2a8f6e5256957c029cf3a8912d51438e7faa5891c5c102c312f6d4599c1f00",
    "Cargo.toml.orig": "90923641c7cd59e9f3df14ab37d39523d4a6e59cf02d7a6aae79abcaca07e3b9",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "378f5840b258e2779c39418f3f2d7b2ba96f1c7917dd6be0713f88305dbda397",
    "README.md": "2406e83ee174e30aa67f8ab266836fa78545012b196395aff37c152321e2c713",
    "src/lib.rs": "54b0f108b0dc48a077c52c0bcd22b64ef4de083e5e2b7d405e50ae4d78224f1b",
    "tests/xcrate.rs": "c0734dae6e63beafcd60bf53546115a2320735b51035c9e2387fdf9301580934"
  },
  "independent_context_files": [],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "be736ec251c13de929f1d0716d10c6e021c1588466edd33443e0d144837f0ced",
  "primary_report_sha256": "8f8113da50f278f42575a4bf361379e4b388527fb71c1e199f6eaa76b068bae7",
  "cross_report_sha256": "3da1905e4d801141ee9e083ea0fc22c20a860f6d62d3aade0e157faff32dec29"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# cfg-if 1.0.4 source review

Target archive SHA-256 `9330f8b2ff13f34540b44e946ef35111825727b38d33286ef986142615121801`, checked against frozen Cargo.lock and registry index. See `../provenance.json` for member safety, checksum inputs and complete inventory.
Required `safe-to-deploy`. Reviewer `Codex (GPT-6.1 Terra; AI-assisted source review)`.

Base `1.0.0` SHA-256 `baf1de4339761588bc0619e3cbc0120ee582ebb74b53b4efbf79117bd2da40fd`; directed baseline certificate chain and source hashes in `../provenance.json`.
Complete reviewed diff: `../diffs/cfg-if-1.0.0-to-1.0.4.diff` SHA-256 `be736ec251c13de929f1d0716d10c6e021c1588466edd33443e0d144837f0ced`.

## Read-file list (old SHA-256 -> new SHA-256)

- `.cargo_vcs_info.json` `adc7a2b67bc53600a229c2892d592977aa4939434fb93f53b2e98799173a675c` -> `65840ba044457862e25b2d7d53f4a2de232adf933bd31aba8f2bd6a1f51a6881`
- `.github/dependabot.yml` `absent` -> `828e3ecefdc555a5210a5bdffd5621ef3625ceb35c7fc91a0b4faef6f9921b75`
- `.github/workflows/main.yaml` `absent` -> `6612a51b1f1479eabac7d3bd14aa609811d4afd2df2d454b9a1f6d6f3748f5b2`
- `.github/workflows/main.yml` `27a418e9130e6462907cf246c5b71c9d51bc684873219c561011928dd829d445` -> `absent`
- `.github/workflows/publish.yaml` `absent` -> `1417805078704eecbaeea8611c5a44df575bfe1908d6969d909224a6e5e26ca8`
- `CHANGELOG.md` `absent` -> `08ba7340057565b338afaa29b36bd2a1c46f5495b043bc49d12230a6a82d5f76`
- `Cargo.lock` `absent` -> `26922b9384045e5a3d496f21ec7c355da585d0caa1d13b887b634527d36fc450`
- `Cargo.toml` `5b2a8f6e5256957c029cf3a8912d51438e7faa5891c5c102c312f6d4599c1f00` -> `281d508beb1fe3927cf03d3f2f8c9a5117b1e4fe97ae21b9026cf318e8c35273`
- `Cargo.toml.orig` `90923641c7cd59e9f3df14ab37d39523d4a6e59cf02d7a6aae79abcaca07e3b9` -> `5a17ee17da78f5179373b8324a1180e71efe2bcf3e3c9ca18c1bdb1e3faa9792`
- `README.md` `2406e83ee174e30aa67f8ab266836fa78545012b196395aff37c152321e2c713` -> `cd565d563a2c70d197bb6fee1678e122214e22af7bdb046b80f52c1d953cd72f`
- `src/lib.rs` `54b0f108b0dc48a077c52c0bcd22b64ef4de083e5e2b7d405e50ae4d78224f1b` -> `c09723e0890d15810374009e96b20bf0eb2f65f383006516f34db36240835c85`
- `tests/xcrate.rs` `c0734dae6e63beafcd60bf53546115a2320735b51035c9e2387fdf9301580934` -> `bcec148e69db81b1a618bdd6f96a25d9a0442e6ecc692fe28f1206d9bffc006a`

## Analysis

Read complete 1.0.0 -> 1.0.4 diff for all 12 paths, including both workflows, Cargo.lock, both Cargo.toml files, README and tests. Bytecode Alliance's fixed-commit full 1.0.0 record covers safe-to-deploy; see exact author and checksum chain in provenance.json. Manifest adds declared rust-version 1.32 and build=false, disables auto-discovery then explicitly identifies lib/test, removes optional compiler_builtins and its rustc-dep-of-std feature member while retaining optional rustc-std-workspace-core. No new build script, native/binary file, proc-macro or runtime dependency is introduced. Added upstream workflow has release token and cargo-registry-token steps, and removed workflow had a docs publish token: these are GitHub workflow context, not commands or secret reads executed by downstream Cargo builds.

src/lib.rs macro changes from two top-level matchers to one optional else and tt-based cfg tokens (supports cfg(true/false)). `@__items` emits each branch with `cfg(all(current, not(any(previous))))`, then accumulates previous conditions; the final else has no current condition and is excluded when any earlier condition matches. Grouping via `@__temp_group` applies cfg to all tokens, preventing a branch with multiple items from having only its first item gated; test issue #90 covers this. Emitted tokens are the caller's tokens, so their own unsafe, IO, etc. remain caller responsibility; the macro introduces no unsafe, FFI, network, filesystem, env or concurrency. Publicly callable undocumented internal `@__identity` is renamed, which may break consumers relying on internals but is not a hidden capability.

Criteria: no serious security vulnerability identified in this delta for safe-to-deploy (and safe-to-run), conditional on imported base audit; dependency internals not audited. No full cross-compiler feature matrix or formal macro proof was run.

## Proposed delta audit (not registered)

```toml
[[audits.cfg-if]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "1.0.0 -> 1.0.4"
```


## Independent cross-review

src/lib.rs:28-95 replaces the matcher and internal grouping: each branch emits all(current, not(any(previous))) and groups all supplied items through @__temp_group so cfg applies to a whole branch; final else negates prior branches. This preserves first-match selection for the inspected forms. The previously exported but undocumented @__identity internal arm is removed, a potential compatibility boundary for callers relying on it. Manifest removes compiler_builtins from the rustc-dep-of-std feature; release CI can use tokens but is not a Cargo dependency-build action. All 12 changed paths were reviewed. Bytecode Alliance full safe-to-deploy at 1.0.0 anchors the proposed 1.0.0 -> 1.0.4 delta.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
