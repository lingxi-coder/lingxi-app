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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/Cargo.toml) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/rust-toolchain.toml) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/apps/setup.sh) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/ec6ff963545c8d02e0c9c220a22f07874ba62959/scripts/lib/runtime_source.py) — Committed source (ec6ff963545c8d02e0c9c220a22f07874ba62959)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/98fc362da3fabfe36aa5610b9e93818923c1ab31/Cargo.toml) — Committed source (98fc362da3fabfe36aa5610b9e93818923c1ab31)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/46240a65df198a654d7d6de1c0bd99c23fcbb979/Cargo.toml) — Committed source (46240a65df198a654d7d6de1c0bd99c23fcbb979)

- lingxi-app: HEAD ec6ff963545c8d02e0c9c220a22f07874ba62959; Referenced input fingerprint sha256:b657a9b5e03a337d3dce34527025cc7cd87afb3439754fe074426c39589fd716
- harness-runtime: HEAD 98fc362da3fabfe36aa5610b9e93818923c1ab31; Referenced input fingerprint sha256:8ac34e7055ee1007cfccf0eafbeaa5e2437c2f20dc623f1069d255177d27dd90; Product pin 98fc362da3fabfe36aa5610b9e93818923c1ab31
- llm-client: HEAD 46240a65df198a654d7d6de1c0bd99c23fcbb979; Referenced input fingerprint sha256:548db437ccaaeec3ce6ad3beb931ef868b263f6ad9b3c47309a3e00c514326dd; Product pin 46240a65df198a654d7d6de1c0bd99c23fcbb979

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
