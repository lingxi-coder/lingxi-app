# Development environment & dependency identity

Set up the four repositories, build incrementally, and make SDK changes without silently mixing local and pinned Rust types.

<a id="layout"></a>

## Local layout and prerequisites

Keep lingxi-app, harness-runtime, llm-client and mobile-linux-runtime as sibling checkouts. Rust is fixed by rust-toolchain.toml; JavaScript packages use npm and their package-lock.json; the current Vite/Electron Vite toolchain requires Node ^20.19.0 or >=22.12.0. Native builds additionally need their Gradle/NDK or Xcode toolchain. SDK and product tests stay in their own repositories.

### Product bootstrap

```sh
cd lingxi-app
./apps/setup.sh
# Website/documentation dependencies
npm --prefix apps/web ci
```

Verification method: Environment validation

<a id="identity"></a>

## Resolve the actual product SDK

The source resolver uses locked Cargo metadata, canonical URLs, full revisions and package containment. It rejects duplicate or unintended identities. Use the resolver rather than guessing paths in Cargo caches. The nine-member product workspace includes apps/audio-provider; default members select desktop product packages.

### Inspect fixed sources

```sh
cd lingxi-app
python3 scripts/lib/runtime_source.py --root
python3 scripts/lib/mobile_linux_source.py --root
cargo metadata --locked --format-version=1 --all-features
```

Verification method: Environment validation

<a id="overlay"></a>

## Working-tree SDK consumers

Current-worktree examples require local checkouts and an explicit complete dependency overlay. A path dependency on Harness alone does not replace its Git-pinned LLM and Mobile Linux edges. The example validator creates an isolated snapshot, patches matching source URLs and checks every SDK package path; it leaves production manifests and locks untouched. Offline validation seeds the isolated consumer lock from the SDK lockfile to retain cached third-party versions.

- [Harness consumer manifest](harness.md#installation)
- [Example validation and source checks](validation-evidence.md)

<a id="incremental"></a>

## Incremental platform changes

- Build bridge-client before Electron because its exports resolve to dist. Rust Bridge changes require rebuilding the binary; renderer-only changes can reuse it.
- FFI signature changes require regenerating native bindings before compiling Kotlin or Swift. Pure native UI changes can reuse compatible bindings.
- Translations and voice settings have canonical sources under resources; run their generators and checks instead of editing generated native output.

<a id="upgrade"></a>

## Dependency upgrades

1. Validate the changed SDK in isolation and review its public contract.
2. After the SDK has a reviewed immutable revision, update all consumer dependencies for that repository to one canonical URL and revision.
3. Review lockfile and source resolver results, then regenerate affected bindings, API catalog and handbook baselines.
4. Run focused integration checks before packaging or platform acceptance.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/9b4abdd5d66862ecae244e4180508d88a4cb67d9/Cargo.toml) — Committed source (9b4abdd5d66862ecae244e4180508d88a4cb67d9)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/9b4abdd5d66862ecae244e4180508d88a4cb67d9/rust-toolchain.toml) — Committed source (9b4abdd5d66862ecae244e4180508d88a4cb67d9)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/9b4abdd5d66862ecae244e4180508d88a4cb67d9/apps/setup.sh) — Committed source (9b4abdd5d66862ecae244e4180508d88a4cb67d9)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/9b4abdd5d66862ecae244e4180508d88a4cb67d9/scripts/lib/runtime_source.py) — Committed source (9b4abdd5d66862ecae244e4180508d88a4cb67d9)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/1129e10ad0c8bc3c27e577245165e8290c7e2fec/Cargo.toml) — Committed source (1129e10ad0c8bc3c27e577245165e8290c7e2fec)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/Cargo.toml) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- lingxi-app: HEAD 9b4abdd5d66862ecae244e4180508d88a4cb67d9; Referenced input fingerprint sha256:3347f2641bfc3fafbc7595c66a1dac1b1b7db2f0851b8df8ccc3af1088259fd8
- harness-runtime: HEAD 1129e10ad0c8bc3c27e577245165e8290c7e2fec; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin 1129e10ad0c8bc3c27e577245165e8290c7e2fec
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
