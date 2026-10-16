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

- [lingxi-app/Cargo.toml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/Cargo.toml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/rust-toolchain.toml](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/rust-toolchain.toml) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/apps/setup.sh](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/apps/setup.sh) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [lingxi-app/scripts/lib/runtime_source.py](https://github.com/lingxi-coder/lingxi-app/blob/cc3f725e248e4d717ceecec4c029a86a318dec3e/scripts/lib/runtime_source.py) — Committed source (cc3f725e248e4d717ceecec4c029a86a318dec3e)
- [harness-runtime/Cargo.toml](https://github.com/lingxi-coder/harness-runtime/blob/6a26289330604ca621d3d1c46e5e71e3369f9955/Cargo.toml) — Committed source (6a26289330604ca621d3d1c46e5e71e3369f9955)
- [llm-client/Cargo.toml](https://github.com/lingxi-coder/llm-client/blob/7333dbc3795b6bad893cb7287088d80931538630/Cargo.toml) — Committed source (7333dbc3795b6bad893cb7287088d80931538630)

- lingxi-app: HEAD cc3f725e248e4d717ceecec4c029a86a318dec3e; Referenced input fingerprint sha256:af041726ea2dcbfff18678637efb08857c8817c356f2a3bf2e0ddd2bd1205b1e
- harness-runtime: HEAD 6a26289330604ca621d3d1c46e5e71e3369f9955; Referenced input fingerprint sha256:50eb3dc51cb2a5ee927dcf0f48b20b7e956398fe44c92c11c0f1ee8b4b25cc83; Product pin 6a26289330604ca621d3d1c46e5e71e3369f9955
- llm-client: HEAD 7333dbc3795b6bad893cb7287088d80931538630; Referenced input fingerprint sha256:51afdece5bf1e2d135f676f447d6ff43c84aaa464328f96ba19fa1e5010928af; Product pin 7333dbc3795b6bad893cb7287088d80931538630

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
