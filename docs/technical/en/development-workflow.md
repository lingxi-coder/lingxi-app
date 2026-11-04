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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/Cargo.toml) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/rust-toolchain.toml) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/apps/setup.sh) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87/scripts/lib/runtime_source.py) — Committed source (4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/f07f87e9a179379e6c429d6143007f979500bd81/Cargo.toml) — Committed source (f07f87e9a179379e6c429d6143007f979500bd81)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/66ce76a2ecc679f0dba551e26ef6aaceb40d2f49/Cargo.toml) — Committed source (66ce76a2ecc679f0dba551e26ef6aaceb40d2f49)

- lingxi-app: HEAD 4c38c5f194b7fd0864d18e2f85e0ae4bd83f7e87; Referenced input fingerprint sha256:40b76956c72557655c95a9de37a4fed9d6afad2dc25a52556454f55b9fc0a60a
- harness-runtime: HEAD f07f87e9a179379e6c429d6143007f979500bd81; Referenced input fingerprint sha256:c3dfae4cc5eb31ce91b7b2090c819842f550bbe7ebb0d564931735752ad375b2; Product pin e54f6d018e334dec3153e11d67f8a8224ff538d2
- llm-client: HEAD 66ce76a2ecc679f0dba551e26ef6aaceb40d2f49; Referenced input fingerprint sha256:89fef6f87fb7803bec69d768fa11e408a54f2123d196e2c859d8faba79092d96; Product pin 3cb37e3a3b840b54a4e92994fdc8b1edf7f592d3

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
