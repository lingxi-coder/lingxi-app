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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/Cargo.toml) — Committed source (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/rust-toolchain.toml) — Committed source (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/apps/setup.sh) — Committed source (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/bf12d8217b18d91a37ae51ed90a9be0b975fb9b0/scripts/lib/runtime_source.py) — Committed source (bf12d8217b18d91a37ae51ed90a9be0b975fb9b0)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/d3165b04f670c898b75c90732af8907e28b05bd5/Cargo.toml) — Committed source (d3165b04f670c898b75c90732af8907e28b05bd5)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/Cargo.toml) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- lingxi-app: HEAD bf12d8217b18d91a37ae51ed90a9be0b975fb9b0; Referenced input fingerprint sha256:be15e9524b52d312f415a56fb611a59c4c0e73f5a858b8c268772f4532863b2d
- harness-runtime: HEAD d3165b04f670c898b75c90732af8907e28b05bd5; Referenced input fingerprint sha256:e9079905b280000c8013dd7f16954f3709e464fdcf67fca25de7e73176e1230a; Product pin d3165b04f670c898b75c90732af8907e28b05bd5
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
