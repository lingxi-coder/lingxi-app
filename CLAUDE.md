# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this is

LingXi's product repository: CLI/TUI, desktop and mobile hosts, native clients and
product resources. The shared agent runtime is a pinned Git dependency from
`lingxi-coder/harness-runtime`, not a local collection of engine crates.

## Repo navigation

- `lingxi-code/` — ten Rust workspace members: product entrypoints, TUI and host adapters.
- `clients/` — Electron, iOS, Android, Web, shared TypeScript SDK, translations and voice configuration.
- `docs/` — current architecture and guides; historical notes are identified in `docs/README.md`.
- `third_party/` — mksh/toybox source required by Android builds.
- `assets/brand/` — design source assets; absence of a runtime import does not make them disposable.
- `scripts/`, `.github/` — repository tooling and CI.

Root `claude-code/`, `codex/`, `backups/`, `output/` and code-intelligence indexes
are ignored local material. Scope sweeps to tracked product files; do not recurse
into reference checkouts or delete user state and signed release artifacts.

## Build and test

### Engine (from `lingxi-code/`)

```bash
cargo build --locked -p cli -p bridge-server --release
cargo test --locked --workspace --all-features --no-fail-fast   # what CI runs
cargo test --locked -p bridge-server                   # retained product crate
./scripts/check-all.sh                                  # every checked-in gate
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
```

`check-all.sh` **discovers** gates by walking `scripts/` for `check-*.sh` / `*-gate.sh` — a new gate is picked up without editing a list. Run it from `lingxi-code/`, not from inside `scripts/`.

Resolve upstream sources with `python3 scripts/runtime_source.py --root`. Runtime crates and their own unit-test suites are maintained upstream; local workspace tests cover the retained product packages.

### Clients

```bash
./clients/setup.sh          # idempotent bootstrap for a fresh clone
```

`clients/shared` (`@lingxi/bridge-client`) **must be built before electron** — electron resolves it via `file:../shared` and consumes the compiled `dist/`.

```bash
cd clients/shared  && npm install && npm run build
cd clients/electron && npm install && npm run typecheck && npm test
node --import ../shared/node_modules/tsx/dist/loader.mjs --test test/<one>.test.ts
```

Mobile builds are optional in `setup.sh` and skip cleanly without their toolchains: Android needs `cargo-ndk` + NDK (`clients/android/scripts/build-jni.sh`); iOS needs `xcodebuild` + `xcodegen` (`clients/ios/scripts/build-xcframework.sh`, then `xcodegen generate`).

## Architecture

### Product and runtime ownership

`apps/cli`, `apps/bridge-server`, `apps/ios-framework` and `apps/android-aar`
consume the fixed upstream `harness-runtime` desktop/mobile profiles. Shared
orchestration, tools, permissions, sessions and protocol types live upstream.
The local `scripts/check-runtime-dependency.sh` verifies the common Git identity;
`check-client-protocol.sh` verifies host boundaries and mirrored fixtures.

See `docs/architecture/harness-runtime-extraction.md` for the source and resource
contract. Do not edit Cargo's upstream checkout or add local path overrides.

### Engine ↔ client seam

The clients never see engine types. The path is:

```
orchestrator → client-adapter (lowering) → client-protocol DTOs → bridge-server / UniFFI → client
```

- `client-protocol` — the wire DTOs. `CLIENT_PROTOCOL_VERSION` is governed by a **structural** guard (`tests/version_guard_test.rs`): a removed, renamed, or retyped variant/field is a MAJOR bump; a new variant or new optional field is additive and needs none.
- `client-adapter` — lowers engine values to DTOs. Note the **live vs replay asymmetry**: a live `ToolUseResult` carries the tool's full structured `data`, while a replayed `ContentBlock::ToolResult` holds only model-facing text. Anything structural a client reads off `result_json` must be recovered explicitly on the resume path.
- Desktop reaches the engine over a loopback `bridge-server`; mobile links it in-process via UniFFI.

### Cross-cutting invariants

- No `tokio::spawn` outside the runtime trait — background work goes through `RuntimeSpawner`.
- No `tokio::fs` / `std::fs` in engine crates — all I/O through the `FileSystem` trait.
- `Secret<T>`'s Debug is always `<redacted>`.
- `ProcessRunner::run` accepts only a `SandboxedCommand` — the sandbox is type-enforced.

Key flows (turn loop, tool dispatch, compaction, subagent spawn) are sketched in `docs/ARCHITECTURE.md#key-flows`.

## macOS Electron packaging

From `AGENTS.md`, which governs this and should be read in full before packaging:

- Always package through `npm run package:mac:flare` (or `scripts/package-macos-flare.sh`) from `clients/electron`. Do **not** bypass the wrapper with `npm run package:mac`.
- Team: Flare App, Inc., `AZ4AX7J833` — the same as iOS. The wrapper rejects a different Team ID and selects the valid `Apple Development` identity by certificate hash.
- Development packaging uses the isolated `development` credential channel and needs **Mac** App Development profiles for `com.lingxi.code.development` and `com.lingxi.code.credential-broker.development`. An iOS Xcode Managed Profile is not a substitute.
- Missing profiles are created through the checked-in provisioning bootstrap target with Xcode Automatic Signing (`-allowProvisioningUpdates -allowProvisioningDeviceRegistration`) — never by automating the Apple Developer website.
- `npm run package:mac:flare -- --check` validates signing assets without a real build; add `-- --launch` to launch after verification.
- Never commit Apple IDs, app-specific passwords, private keys, App Store Connect API keys, or local profile files.

**`npm run dev` cannot test credential persistence or open a session** — the Credential Broker deliberately rejects unpackaged Electron processes and fails closed (no plaintext or login-keychain fallback). Anything touching real credentials or sessions needs a signed package.

## Things that will cost you time

- **`cargo check` is blind to test modules.** Use `cargo build --tests --keep-going` to surface every error at once, and `--all-features` — a feature-gated subsystem (mobile UniFFI) is invisible without it.
- **Generated artifacts are not sources.** `clients/ios/Generated/` and the Android JNI bindings are gitignored build output — change a wire DTO without regenerating and the clients cannot compile. `clients/ios/Resources/Localizable.xcstrings` and Android `strings.xml` are generated from `clients/translations/*.json` by `generate.py`; edit the JSON.
- **`client-protocol --features uniffi` has a metadata budget.** The upstream runtime suite runs `cargo test -p client-protocol --features uniffi --test uniffi_metadata_budget_test`; blowing it breaks both mobile builds while ordinary gates stay green.
- **A green test run is not a green commit.** Uncommitted changes can make the working tree compile while the committed tree does not; and a falling test *count* at zero failures means a binary aborted early, not that everything passed.
