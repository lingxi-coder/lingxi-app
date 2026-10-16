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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/Cargo.toml) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/rust-toolchain.toml) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/apps/setup.sh) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/a7eacf9587076dfd9571b17f6640bc4ead1bba6b/scripts/lib/runtime_source.py) — Committed source (a7eacf9587076dfd9571b17f6640bc4ead1bba6b)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/e3e34efba151a8219e2b05406c3d229b5cc6314f/Cargo.toml) — Committed source (e3e34efba151a8219e2b05406c3d229b5cc6314f)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/a363c5b3a237d53eea12c3abfb3b98648ef15d3e/Cargo.toml) — Committed source (a363c5b3a237d53eea12c3abfb3b98648ef15d3e)

- lingxi-app: HEAD a7eacf9587076dfd9571b17f6640bc4ead1bba6b; Referenced input fingerprint sha256:b37c12336706c41006c29d34cf09f0c9a109330ac15afacc8fa7a858e2df4eab
- harness-runtime: HEAD e3e34efba151a8219e2b05406c3d229b5cc6314f; Referenced input fingerprint sha256:2d439c83e9c12bcb5aacfcf0a5da70a9d8beba68e9eba861d817bcf67c9bb51b; Product pin e3e34efba151a8219e2b05406c3d229b5cc6314f
- llm-client: HEAD a363c5b3a237d53eea12c3abfb3b98648ef15d3e; Referenced input fingerprint sha256:fdc3f37adcc11c571e4db361c4ea665e8ee4d2644a8724745b22389fb43c41d7; Product pin a363c5b3a237d53eea12c3abfb3b98648ef15d3e

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
