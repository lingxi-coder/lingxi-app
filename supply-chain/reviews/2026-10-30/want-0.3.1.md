# Source review: want 0.3.0 -> 0.3.1

Reviewer: Codex (GPT-6.1 Terra; AI-assisted source review). Independent cross-review: Codex AI-assisted.
Criterion: `safe-to-deploy`; scope: delta.
Frozen batch SHA-256: `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43`; frozen original Cargo.lock SHA-256: `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d`.
This review applies to the frozen versions only; the final dependency graph can differ. Each dependency remains separately gated.

## Archive, registry and audit-chain evidence

```json
{
  "target": {
    "version": "0.3.1",
    "archive_sha256": "bfa7760aed19e106de2c7c0b581b509f2f25d3dacaf737cb82ac61bc6d760b0e",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "9f24a3791796ba4ae431633522fc7f6e62a7c6d264446641e90e0d432aaeb951",
        "index_record_sha256": "77a56749179b63b9875e203489d3aa0e59b2242a13a425f8b26c6c6cecec931e",
        "cksum": "bfa7760aed19e106de2c7c0b581b509f2f25d3dacaf737cb82ac61bc6d760b0e",
        "yanked": false
      }
    ]
  },
  "base": {
    "version": "0.3.0",
    "archive_sha256": "1ce8a968cb1cd110d136ff8b819a556d6fb6d919363c61534f6860c7eb172ba0",
    "index": [
      {
        "registry": "crates.io sparse index",
        "index_file_sha256": "965886522c4a59c0b225323b315e3bd6118e45f8fb71a36f892ae9518ebca065",
        "index_record_sha256": "1ec7b6e937478d6a70e9221b94124d225b67ab73dcb272919480fe1e35baea5d",
        "cksum": "1ce8a968cb1cd110d136ff8b819a556d6fb6d919363c61534f6860c7eb172ba0",
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
        "version": "0.3.0"
      }
    }
  ],
  "read_files": [
    {
      "role": "baseline",
      "path": ".cargo_vcs_info.json",
      "sha256": "32e4641c9b3da296674f2993950979547f721dd5b5704336a16d0e950d060200",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml",
      "sha256": "aa3ff35ee3a13df045f467b87cca65a8cc3bf05fd390a1347f0d2d9673894708",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "Cargo.toml.orig",
      "sha256": "57a008030d73e7422066e6acf861146228c315c8da8ec67b215aba6db45b373f",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "baseline",
      "path": "src/lib.rs",
      "sha256": "2d5bf55eae742f96b490838e079b05c9efb2765e7e6aa72d7fa31baa448a0fc0",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": ".cargo_vcs_info.json",
      "sha256": "429ab0ce9b8ba8cc7e1ea741ff0d86e364a5e1ab7189a5258cbf011d34b8ba13",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml",
      "sha256": "41b6f47b839dfb51337ca7b84a7bd7d0723619ddb5230b64f9f31b76b9ac0a57",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "Cargo.toml.orig",
      "sha256": "cba3bd08e35bdc00457ab3cdd036140ff52becab251cd35605adaac92f8c5ad5",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    },
    {
      "role": "target",
      "path": "src/lib.rs",
      "sha256": "bc0fbf3ef74ae1020acb54098923c852b14a4e53578640d767b6398bc08b3c1e",
      "scope": "whole changed file; base reconstructed from target plus full delta where appropriate"
    }
  ],
  "target_inventory_sha256": {
    ".cargo_vcs_info.json": "429ab0ce9b8ba8cc7e1ea741ff0d86e364a5e1ab7189a5258cbf011d34b8ba13",
    ".gitignore": "d6b00dd423fb9326e9995b0f0cdf67909e5efd3947d09ce863425bd7edc40ebf",
    "Cargo.toml": "41b6f47b839dfb51337ca7b84a7bd7d0723619ddb5230b64f9f31b76b9ac0a57",
    "Cargo.toml.orig": "cba3bd08e35bdc00457ab3cdd036140ff52becab251cd35605adaac92f8c5ad5",
    "LICENSE": "a65f5d0a945d267751344c95665945b90c030ea107faf5c85d518929886187da",
    "README.md": "316ed49131b031a3b323a3d4eab8327e4207ff2728c661c3c72556b63457038b",
    "benches/throughput.rs": "d183d648db263d8b82fa9496132820d8ab56bacc637a06c7c8746e4fadaefb49",
    "src/lib.rs": "bc0fbf3ef74ae1020acb54098923c852b14a4e53578640d767b6398bc08b3c1e"
  },
  "base_inventory_sha256": {
    ".cargo_vcs_info.json": "32e4641c9b3da296674f2993950979547f721dd5b5704336a16d0e950d060200",
    ".gitignore": "d6b00dd423fb9326e9995b0f0cdf67909e5efd3947d09ce863425bd7edc40ebf",
    "Cargo.toml": "aa3ff35ee3a13df045f467b87cca65a8cc3bf05fd390a1347f0d2d9673894708",
    "Cargo.toml.orig": "57a008030d73e7422066e6acf861146228c315c8da8ec67b215aba6db45b373f",
    "LICENSE": "a65f5d0a945d267751344c95665945b90c030ea107faf5c85d518929886187da",
    "README.md": "316ed49131b031a3b323a3d4eab8327e4207ff2728c661c3c72556b63457038b",
    "benches/throughput.rs": "d183d648db263d8b82fa9496132820d8ab56bacc637a06c7c8746e4fadaefb49",
    "src/lib.rs": "2d5bf55eae742f96b490838e079b05c9efb2765e7e6aa72d7fa31baa448a0fc0"
  },
  "independent_context_files": [
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/want-0.3.1/scratch/target/src/lib.rs",
      "sha256": "bc0fbf3ef74ae1020acb54098923c852b14a4e53578640d767b6398bc08b3c1e"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/want-0.3.1/scratch/target/Cargo.toml",
      "sha256": "41b6f47b839dfb51337ca7b84a7bd7d0723619ddb5230b64f9f31b76b9ac0a57"
    },
    {
      "role": "target",
      "path": "validation/gates-fix-20261030/source-audits/lane-a-001/want-0.3.1/scratch/target/Cargo.toml.orig",
      "sha256": "cba3bd08e35bdc00457ab3cdd036140ff52becab251cd35605adaac92f8c5ad5"
    }
  ],
  "external_interface_context": [
    {
      "path": "crates.io/try-lock-0.2.5/src/lib.rs",
      "sha256": "4f8e19b938527c5543e3180901a11ec31f8933d5ac918790456d2f4027a12f6f",
      "scope": "lines 98-175; source equals locked, registry-index-verified 0.2.5 archive e421abadd41a4225275504ea4d6566923418b7f05506fbc9c0fe86ba7396114b"
    }
  ],
  "dependency_interface_contracts": null,
  "full_diff_sha256": "313d45aeac3f4b8a6cd7a99e2e8a67a318db96c691b71bbf9667de615260319d",
  "primary_report_sha256": "cf1f385b828c2516b60005ea7058b41f4388cac282c5ad07d814519499032e4c",
  "cross_report_sha256": "4c756225d6eff9f09f0eca24e608e2518e571ddeced5db7825cd05ee6d83534a"
}
```

Inventory hashes identify all archive members; only read-file entries and the complete reviewed delta claim review coverage. Source chain records retain original attribution; the new review is AI-assisted.

## Primary review

# want 0.3.1: source delta review

**Disposition:** review complete; candidate `safe-to-deploy` delta for root review. Auditor: Codex (GPT-6.1 Terra; AI-assisted source review). No record has been registered.

## Identity and evidence

- Frozen batch `aa0f8805f0e48d23f3eeae37a1e4097c9bdd761e30801099b4d1c3eded471b43` and product Cargo.lock `2a60d89a9fb754475a33d8034227743a8bb5ff2e7410b064f5c122800360385d` are verified. Target archive `bfa7760aed19e106de2c7c0b581b509f2f25d3dacaf737cb82ac61bc6d760b0e` matches the product-lock and registry-index checksums; base `0.3.0` archive `1ce8a968cb1cd110d136ff8b819a556d6fb6d919363c61534f6860c7eb172ba0` matches its registry index. Manifest identities agree.
- `verification.json` records archive/index paths, member hashes and types, and original records of the trusted safe-to-deploy baseline links. Source snapshots match their fixed SHA-256 and Git blob IDs. No links, special entries, path escape, case/Unicode filename collisions, NUL-containing binary, or other non-UTF-8 member was found. Extra checks are recorded in `../member-safety-extra.json`.
- `full.diff` is the complete package diff with SHA-256 `313d45aeac3f4b8a6cd7a99e2e8a67a318db96c691b71bbf9667de615260319d`; its changed-file set matches the frozen queue. `read-files.json` lists reviewed changed files, context and externally read dependency-interface source with hashes. Unchanged files are compared by bytes; no independent full audit of the certified base is claimed.

## Analysis

- `src/lib.rs:192-240,243-255,340-361` replaces SeqCst compare_and_swap with strong compare_exchange using SeqCst on success and failure; testing `Ok(expected)` preserves the previous success predicate ([Rust atomic migration table](https://doc.rust-lang.org/std/sync/atomic/struct.Atomic.html)). Failed CAS retries; the locked waker is updated only after success. The old Option::map wake and new if-let wake do the same work and release the guard before invoking the old waker; changes to lifetime spelling and struct shorthand do not alter behavior.
- Both polling and signal paths change `try_lock_order(SeqCst, SeqCst)` to `try_lock_explicit(SeqCst, SeqCst)`. The frozen lock's verified try-lock 0.2.5 source lines 98-175 routes the deprecated former and the latter to the same unchecked operation, with the latter first validating orderings; SeqCst passes those checks. The state machine still uses SeqCst and releases the task lock before waking a task. This is not a formal interleaving proof; try-lock remains a separate audit.
- `Cargo.toml:28-35` removes the logging dependency, raises try-lock minimum from 0.2 to 0.2.4, retains dev-only Tokio requirements, and declares no build script or MSRV. Removed trace macros reduce logging; no added file, network or process operation is present.

## Criteria and boundaries

Every authenticated baseline link explicitly carries safe-to-deploy, which includes safe-to-run ([cargo-vet criteria](https://mozilla.github.io/cargo-vet/built-in-criteria.html)). In this reviewer’s judgment the delta preserves those criteria. Progress still depends on the inherited atomic state machine and independently reviewed try-lock. Spin/retry, wake-on-replacement, cancellation and potential callback panics are inherited; no concurrency model checking or runtime graph test was run. Dependent crates must satisfy their own audits. This static review is neither formal verification nor cross-platform runtime acceptance; no product, lock, registry or audit store was changed.

## Proposed record, for root review only

```toml
[[audits.want]]
who = "Codex (GPT-6.1 Terra; AI-assisted source review)"
criteria = "safe-to-deploy"
delta = "0.3.0 -> 0.3.1"
notes = "Full delta reviewed: SeqCst CAS and try-lock migration preserve state transitions; removed trace logging; try-lock separately audited."
```


## Independent cross-review

Read the entire 0.3.0 -> 0.3.1 delta and relevant poll/signal code. In src/lib.rs:192-240, 247-255, 340-360, compare_and_swap(expected, new, SeqCst)==expected becomes strong compare_exchange(expected, new, SeqCst, SeqCst)==Ok(expected): success and failure retain SeqCst ordering. try_lock_order(SeqCst,SeqCst) becomes try_lock_explicit(SeqCst,SeqCst), with try-lock minimum 0.2 -> 0.2.4. Independently read frozen resolved try-lock 0.2.5 source: old deprecated method delegates to unchecked function; new method validates these exact SeqCst orders and delegates to the same function (src/lib.rs:105-145); both wake paths release the guard before wake. Log calls/dependency are removed, and lifetime/if-let changes do not change state transitions. Frozen try-lock 0.2.5 archive SHA e421abadd41a4225275504ea4d6566923418b7f05506fbc9c0fe86ba7396114b matches frozen lock; its src/lib.rs SHA 4f8e19b938527c5543e3180901a11ec31f8933d5ac918790456d2f4027a12f6f. This narrow dependency call inspection is not a full try-lock audit or a concurrency proof. Authenticated bytecode-alliance full 0.3.0 supplies safe-to-deploy baseline.

Static evidence is not a formal proof, exhaustive platform or concurrency execution, or a recursive certification of dependencies.
