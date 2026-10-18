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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3/Cargo.toml) — Committed source (a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3/rust-toolchain.toml) — Committed source (a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3/apps/setup.sh) — Committed source (a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3/scripts/lib/runtime_source.py) — Committed source (a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/6548b9d5e702f8e9d42772412cd65b72881e24b4/Cargo.toml) — Committed source (6548b9d5e702f8e9d42772412cd65b72881e24b4)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/Cargo.toml) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- lingxi-app: HEAD a2a45fd0e845558ce55aa35e6bd9f57ed4491ed3; Referenced input fingerprint sha256:9599eff51b5078b439c85bc3d8d22b4780569bb284e75cc3b493c047a79aa30f
- harness-runtime: HEAD 6548b9d5e702f8e9d42772412cd65b72881e24b4; Referenced input fingerprint sha256:92c43ec1558ef6090bd3e0c28f21110761b7855079f22272b843555f562fe25b; Product pin 6548b9d5e702f8e9d42772412cd65b72881e24b4
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
