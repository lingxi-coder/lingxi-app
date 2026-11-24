# Build, signing & release procedures

Build each product entrypoint and distinguish dependency checks, signed packaging, static artifacts and published releases.

<a id="rust"></a>

## CLI and Bridge

Cargo package selection follows the product manifest. CLI npm/PyPI wrappers distribute the application, not an independent Rust SDK. Release workflow identity and target artifacts must be reviewed together; a successful local binary is not a published package.

### Locked product builds

```sh
cd lingxi-app
cargo build --locked -p cli --release
cargo build --locked -p bridge-server --bin bridge-server
npm --prefix packages/bridge-client ci
npm --prefix packages/bridge-client run build
```

Verification method: Environment validation

<a id="macos"></a>

## Signed macOS application

Use the repository packaging wrapper. It coordinates Bridge, Electron, native helpers, credential broker, provisioning, inside-out signing and packaged static/runtime checks. Credential persistence needs the signed development channel; an unpackaged npm run dev process is not an equivalent acceptance environment.

### Signing preflight and packaged launch

```sh
cd lingxi-app/apps/electron
npm run package:mac:flare -- --check
npm run package:mac:flare -- --launch
```

Verification method: Environment validation

Provisioning and signing may change developer-account or device registration state. Reading these pages does not run packaging or release commands.

<a id="mobile"></a>

## Native packages and resources

Android builds combine Rust FFI, native execution assets, generated Kotlin bindings and Gradle. iOS builds combine Rust static libraries/XCFrameworks, generated Swift bindings, native resources and the Xcode project. Verify ABI slices, SDK/rootfs provenance and binding identity before device tests.

- [Android integration](mobile-android.md)
- [iOS integration](mobile-ios.md)

<a id="website"></a>

## Website and compiler documentation

Rustdoc builds stay in ignored output directories. Development and static-build availability checks remove links when saved ready-profile evidence has no corresponding artifact in that bundle. A static site build supplies documentation and assets; it does not deploy an Agent API or perform release publication. Publishing the website is a separate authorized operation.

### Documentation build

```sh
cd lingxi-app/apps/web
npm run docs:sync
npm run docs:check
npm run docs:examples
npm run docs:rustdoc
npm run typecheck
npm test
npm run build
```

Verification method: Environment validation

<a id="checklist"></a>

## Release evidence

1. Record all SDK/source revisions and artifact checksums.
2. Validate required feature and ABI combinations; record unavailable targets instead of inventing support.
3. Check generated resources, signatures and packaged behavior using the platform workflow.
4. Publish only after release review, then verify remotely published identity and artifacts.

## Sources and documentation baseline

[Source repository](https://github.com/lingxi-coder/lingxi-app)

- [lingxi-app/.github/workflows/lingxi-release.yml](https://github.com/lingxi-coder/lingxi-app/blob/9afa34a6dd5c46de7eb80a3ccbe2d35df9200778/.github/workflows/lingxi-release.yml) — Committed source (9afa34a6dd5c46de7eb80a3ccbe2d35df9200778)
- [lingxi-app/.github/workflows/desktop-packages.yml](https://github.com/lingxi-coder/lingxi-app/blob/04651101dff21184dcb839ef89708c2c0c9c537d/.github/workflows/desktop-packages.yml) — Committed source (04651101dff21184dcb839ef89708c2c0c9c537d)
- [lingxi-app/apps/electron/scripts/package-macos-flare.sh](https://github.com/lingxi-coder/lingxi-app/blob/a499e8c638dfcc8d91420efe6e1b7690b0953546/apps/electron/scripts/package-macos-flare.sh) — Committed source (a499e8c638dfcc8d91420efe6e1b7690b0953546)
- [lingxi-app/apps/web/package.json](https://github.com/lingxi-coder/lingxi-app/blob/ab33bdd823ec03a71e878d1cf8c9f88b3efd39c5/apps/web/package.json) — Committed source (ab33bdd823ec03a71e878d1cf8c9f88b3efd39c5)
- [lingxi-app/apps/web/scripts/build-rustdoc.py](https://github.com/lingxi-coder/lingxi-app/blob/1b3a75958f0fc5bb24b7cca64372abe0464720cb/apps/web/scripts/build-rustdoc.py) — Committed source (1b3a75958f0fc5bb24b7cca64372abe0464720cb)
- [lingxi-app/apps/web/scripts/rustdoc-availability.ts](https://github.com/lingxi-coder/lingxi-app/blob/07ad69d8d219502e63f918d84c3434c97edce8e6/apps/web/scripts/rustdoc-availability.ts) — Committed source (07ad69d8d219502e63f918d84c3434c97edce8e6)
- [lingxi-app/apps/web/vite.config.ts](https://github.com/lingxi-coder/lingxi-app/blob/a499e8c638dfcc8d91420efe6e1b7690b0953546/apps/web/vite.config.ts) — Committed source (a499e8c638dfcc8d91420efe6e1b7690b0953546)
- [lingxi-app/resources/translations/generate.py](https://github.com/lingxi-coder/lingxi-app/blob/a3ecab5a900bae02175bfc9c2bc4507d8f2e6659/resources/translations/generate.py) — Committed source (a3ecab5a900bae02175bfc9c2bc4507d8f2e6659)
- [lingxi-app/apps/ios/native/scripts/build-xcframework.sh](https://github.com/lingxi-coder/lingxi-app/blob/a6ad22cf441750a94165fdfae45de1d2e7428e35/apps/ios/native/scripts/build-xcframework.sh) — Committed source (a6ad22cf441750a94165fdfae45de1d2e7428e35)

- lingxi-app: HEAD e74379a16bb1946787a7edf327cd8367128c4357; Referenced input fingerprint sha256:b2d7aabb4ca87d35400a12fe665e0a43040123560edaadd23961a53bf191fc66

<!-- Generated from apps/web/src/data/docs. Do not edit this file directly. -->
