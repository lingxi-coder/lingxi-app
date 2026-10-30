# Source review: version_check 0.9.4 -> 0.9.5

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.9.5",
    "archive_sha256": "0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "3b59bb407ec049518d98a4baee28599d2724767aad5c95c1b2f25ef511008a3d",
        "index_record_sha256": "0eccbcdcc8bd73baf5208c061bd7f093de14e9279da33f66e0fc026865a00835",
        "cksum": "0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.9.4",
    "archive_sha256": "49874b5167b65d7193b8aba1567f5c7d93d001cafc34600cee003eda787e483f",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "3b59bb407ec049518d98a4baee28599d2724767aad5c95c1b2f25ef511008a3d",
        "index_record_sha256": "583d94b858739a21c940985096c4a1d89df31338b122f565736f9b2256019cf3",
        "cksum": "49874b5167b65d7193b8aba1567f5c7d93d001cafc34600cee003eda787e483f",
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
        "version": "0.9.4",
        "aggregated-from": "https://chromium.googlesource.com/chromiumos/third_party/rust_crates/+/refs/heads/main/cargo-vet/audits.toml?format=TEXT"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "ea958d0b652c03c60b2e7f23e9aaede6435411c474936cf9e93393b17ddd2096",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "f25d88044914cb3466df43bc39a199e1589dda1aad3226c9c7e7ac4d2f8751d0",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "4d6cb51ddf5230afa84b884def590b3958e5002f79ac4aecf25a4fd226ce3afa",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/channel.rs",
      "sha256": "d2443d503d4cc469a171a51a26eca3ec0d2a58b5f7375a84542c36f1421766a8",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "760f0d29567ecaa61287088cf23cf74b3c0efbbcd3077cea5fb7c88359e96c7e",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "fd4d36271280b7893e595f97096f6ff1c9de41d7567c6aa4a684fe7e29e2ce69",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "399ea74400a173c02eabe91cb9487d0cf351117857a5d2ebc6231f0efde67681",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "be3876ab29038ad4ef2ec2d5c625e3b499a9a7bc7b6485fbc57f64e213331ea2",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/channel.rs",
      "sha256": "bb3eae79aaf224591f1e9dddedcabbea3fc01bc6c06eafedcd87d7b770a35aca",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "01bb86088ba281d511ae002aa939bb30b747f47ace5ea13a46de554a3117806e",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "fd4d36271280b7893e595f97096f6ff1c9de41d7567c6aa4a684fe7e29e2ce69",
    ".github/workflows/ci.yml": "0d3ec0059cdc483d7d6d2aaeac4958e95b1a03ad1652f42a38d44cbfac5dcbca",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    ".travis.yml": "d504e08178faaa3a096b0e94981846e9cc0eaf1926c47ef28a4096ae084ddb39",
    "Cargo.toml": "399ea74400a173c02eabe91cb9487d0cf351117857a5d2ebc6231f0efde67681",
    "Cargo.toml.orig": "be3876ab29038ad4ef2ec2d5c625e3b499a9a7bc7b6485fbc57f64e213331ea2",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e",
    "README.md": "ac2a0a360812436bd5798f5fe2affe7d6ed9eb7f15d6e4d73931e95b437560f2",
    "src/channel.rs": "bb3eae79aaf224591f1e9dddedcabbea3fc01bc6c06eafedcd87d7b770a35aca",
    "src/date.rs": "09580a0a2008fad2ccbc43fb42a88f42221b98b01692702022a296dc9c86bf37",
    "src/lib.rs": "01bb86088ba281d511ae002aa939bb30b747f47ace5ea13a46de554a3117806e",
    "src/version.rs": "dba18a25983ec6e37b952f4cdc5219c9e5abba2c3a76cef87465e1fba6f8ac89"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "ea958d0b652c03c60b2e7f23e9aaede6435411c474936cf9e93393b17ddd2096",
    ".github/workflows/ci.yml": "0d3ec0059cdc483d7d6d2aaeac4958e95b1a03ad1652f42a38d44cbfac5dcbca",
    ".gitignore": "f9b1ca6ae27d1c18215265024629a8960c31379f206d9ed20f64e0b2dcf79805",
    ".travis.yml": "d504e08178faaa3a096b0e94981846e9cc0eaf1926c47ef28a4096ae084ddb39",
    "Cargo.toml": "f25d88044914cb3466df43bc39a199e1589dda1aad3226c9c7e7ac4d2f8751d0",
    "Cargo.toml.orig": "4d6cb51ddf5230afa84b884def590b3958e5002f79ac4aecf25a4fd226ce3afa",
    "LICENSE-APACHE": "a60eea817514531668d7e00765731449fe14d059d3249e0bc93b36de45f759f2",
    "LICENSE-MIT": "b7e650f3fce5c53249d1cdc608b54df156a97edd636cf9d23498d0cfe7aec63e",
    "README.md": "ac2a0a360812436bd5798f5fe2affe7d6ed9eb7f15d6e4d73931e95b437560f2",
    "src/channel.rs": "d2443d503d4cc469a171a51a26eca3ec0d2a58b5f7375a84542c36f1421766a8",
    "src/date.rs": "09580a0a2008fad2ccbc43fb42a88f42221b98b01692702022a296dc9c86bf37",
    "src/lib.rs": "760f0d29567ecaa61287088cf23cf74b3c0efbbcd3077cea5fb7c88359e96c7e",
    "src/version.rs": "dba18a25983ec6e37b952f4cdc5219c9e5abba2c3a76cef87465e1fba6f8ac89"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/version_check-0.9.5/scratch/target/src/lib.rs",
      "sha256": "01bb86088ba281d511ae002aa939bb30b747f47ace5ea13a46de554a3117806e"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/version_check-0.9.5/scratch/target/src/channel.rs",
      "sha256": "bb3eae79aaf224591f1e9dddedcabbea3fc01bc6c06eafedcd87d7b770a35aca"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/version_check-0.9.5/scratch/target/Cargo.toml",
      "sha256": "399ea74400a173c02eabe91cb9487d0cf351117857a5d2ebc6231f0efde67681"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/version_check-0.9.5/scratch/target/Cargo.toml.orig",
      "sha256": "be3876ab29038ad4ef2ec2d5c625e3b499a9a7bc7b6485fbc57f64e213331ea2"
    }
  ],
  "external_interface_context": [],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "4d6ffd50729718453e492ed540220c17b28527ce36f15b4492ffcb356ba0493e",
  "primary_report_sha256": "2d71a609f6984e35c748e439dd94e743a1bd8f6bfefd4793c3585b3e8365167e",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# version_check 0.9.5: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `0b928f33d975fc6ad9f86c8f283853ad26bdd5b10b7f1542aa2fa15e2289105a` matches the product-lock and registry-index checksums; base `0.9.4` archive `49874b5167b65d7193b8aba1567f5c7d93d001cafc34600cee003eda787e483f` matches its registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of the trusted safe-to-deploy baseline links. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `4d6ffd50729718453e492ed540220c17b28527ce36f15b4492ffcb356ba0493e`; its changed-file set matches the frozen queue. `read-files.json` lists reviewed changed files, context and externally read dependency-interface source with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/lib.rs:9-105,288-308` and `src/channel.rs:81-84` change documentation: they caution that a nightly feature being available does not guarantee future semantic compatibility. All non-documentation Rust tokens are unchanged in the complete diff; source files for date and version are byte-identical.
- `src/lib.rs:120-164,324-351` continues to read `RUSTC`, `CARGO_ENCODED_RUSTFLAGS` and `RUSTFLAGS`, and to run the configured compiler with separate `--verbose` and `--version` arguments. It does not invoke a shell. Compiler path/environment control and parsing of compiler output are pre-existing trust boundaries, not added in this delta. Test-only code reads excluded `static` files if explicitly present.
- `Cargo.toml:12-38` adds `build = false`, explicit library and disabled automatic non-library targets while retaining an empty dependencies table; original manifest changes version only. The package has no build script; no MSRV is declared. VCS revision and docs are metadata.

## Criteria and boundaries

Every authenticated baseline link explicitly carries safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer’s judgment the delta preserves those criteria. Running a caller-controlled RUSTC is an inherent documented capability; the build environment must be trusted. No new process, filesystem, network, environment or unsafe capability is introduced. No alternate compiler or workspace build was run. Dependent crates must satisfy their own audits. This static review is neither formal verification nor cross-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.version_check]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.9.4 -> 0.9.5"
notes = "Complete delta reviewed; Rust changes are documentation only. Existing RUSTC subprocess and environment trust boundary retained."
```


## Independent cross-review

Read complete 0.9.4 -> 0.9.5 delta, including both manifests and all documentation edits in src/channel.rs and src/lib.rs. The only Rust changes are documentation and link text, not executable statements. Normalized manifest sets build=false, disables automatic binaries/examples/tests/benches and explicitly keeps src/lib.rs; verified archive contains no build.rs or auto-discovered executables. Existing src/lib.rs:160-163 invokes the program named by RUSTC (or rustc from PATH) with --verbose --version, and lines 331-350 inspect CARGO_ENCODED_RUSTFLAGS or RUSTFLAGS; these pre-existing capabilities depend on a trusted build environment and do not change in this delta. No new dependencies, unsafe or FFI. Authenticated google full 0.9.4 provides the safe-to-deploy base; no compiler-execution tests claimed.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
