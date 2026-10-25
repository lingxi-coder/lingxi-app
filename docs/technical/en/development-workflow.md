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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/Cargo.toml) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/rust-toolchain.toml) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/apps/setup.sh) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/77b17c505508c8de002408ca6dddf4c8cf5d2187/scripts/lib/runtime_source.py) — Committed source (77b17c505508c8de002408ca6dddf4c8cf5d2187)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/0e28910fd624638e6c5d1f92eca4899a7938d188/Cargo.toml) — Committed source (0e28910fd624638e6c5d1f92eca4899a7938d188)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b/Cargo.toml) — Committed source (ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b)

- lingxi-app: HEAD 77b17c505508c8de002408ca6dddf4c8cf5d2187; Referenced input fingerprint sha256:542c75ebbb66242ca76885685c120d7913ec9ad0b9b72f6af8865d05c76a6cd0
- harness-runtime: HEAD 0e28910fd624638e6c5d1f92eca4899a7938d188; Referenced input fingerprint sha256:b6b36966a8f4c48b8d0813ea03f73cd617387ddaf2531d8254e4a5ac81f48255; Product pin 0e28910fd624638e6c5d1f92eca4899a7938d188
- llm-client: HEAD ad8c60f7e4ffcbe4c0c0fedc20586df873254c0b; Referenced input fingerprint sha256:c68d002b005c15c21681b54d3f7efe358c4c1be6ec7cb07501009d1d11938e38; Product pin ff71b2ec1139bc67cc8719d78b1735aca967bbe8

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
